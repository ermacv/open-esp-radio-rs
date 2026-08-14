//! Embassy event loop for one connected ESP32-S31 Wi-Fi radio owner.
//!
//! The runner owns PAC/DMA/TX scheduling. Connected-frame protocol state lives
//! in a separate staged-RX consumer, so parsing cannot extend one hardware
//! service epoch. The services owner exposes only finite PAC/DMA transactions: it must
//! never wait for an executor primitive while holding a mutable PAC borrow.

use core::future::{Future, pending, ready};

use embassy_futures::{
    select::{Either, Either3, Either4, select, select3, select4},
    yield_now,
};
use embassy_time::{Duration, Instant, Timer};
use open_esp_radio_embassy_net::{
    PinnedTxConsumer, PinnedTxFrame, RawMutex, SplitPinnedRadioRunner,
};
pub use open_esp_radio_esp32s31_wifi::tx::{WifiTxProgress, WifiTxWake};
pub use open_esp_radio_esp32s31_wifi_sta::connected_control::{
    ConnectedControlContext as WifiControlContext, ConnectedControlProgress as WifiControlProgress,
    ConnectedDisconnectReason,
};

use crate::embassy_irq::EmbassyMacIrqRuntime;

/// Maximum latency added while collecting an aggregate-sized network burst.
/// The target itself comes from the negotiated BlockAck window; this deadline
/// prevents sparse/control traffic from waiting indefinitely for that target.
// At the qualified 100+ Mbit/s offer rate, filling a negotiated 32-MPDU BA
// window from an empty network queue takes roughly 2 ms. Keep that latency
// explicit and bounded: a full window starts immediately, while sparse
// traffic cannot be held beyond this deadline.
const TX_BATCH_MAX_WAIT: Duration = Duration::from_millis(2);
const TX_BATCH_MIN_FRAMES: usize = 2;

/// Result of one bounded RX bottom-half pass.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiRxProgress {
    /// The durable completion frontier was drained within this pass.
    Drained,
    /// Completed descriptors remain, but no independent staging owner is
    /// available. Resume only after protocol processing returns a credit.
    Backpressured,
    /// An exhausted-list BASE publication still needs a cooperative hardware
    /// ownership observation. Re-run RX service without waiting for another
    /// interrupt: the just-republished terminal can complete on the IRQ edge
    /// currently being consumed.
    ProbePending,
}

/// Terminal, non-error outcome of the connected radio event loop.
///
/// Keeping this distinct from `()` prevents an outer station owner from
/// confusing a proved link loss with a runner that completed without a
/// lifecycle transition. The caller may use this edge to tear down the
/// network stack, release the connected epoch and start reassociation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ConnectedRunnerExit {
    /// Connected policy proved that the peer is no longer reachable and the
    /// runner published link-down before returning.
    Disconnected(ConnectedDisconnectReason),
    /// The outer station lifecycle requested a finite stop. The runner waited
    /// for any active TX transaction to release hardware, published link-down
    /// and returned the same owners as a disconnect without claiming peer
    /// reachability had failed.
    Stopped,
}

/// Finite chip-specific operations used by [`ConnectedRunner`].
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
/// and the lifetime of a pinned network lease belong to [`ConnectedRunner::run`].
pub trait ConnectedRunnerServices<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
>
{
    type Error;

    /// Drain one snapshotted RX-success frontier into independent ownership.
    fn service_rx(&mut self) -> impl Future<Output = Result<WifiRxProgress, Self::Error>> + '_;

    /// Apply at most one owned control event or publish one control frame.
    ///
    /// The runner invokes this only while no network TX transaction owns the
    /// shared descriptor. `More` returns through the scheduler before a new
    /// network lease can be claimed; `TxPending` enters the same IRQ/deadline
    /// completion loop as ordinary data.
    fn service_control<'a>(
        &'a mut self,
        _context: WifiControlContext,
    ) -> impl Future<Output = Result<WifiControlProgress, Self::Error>> + 'a
    where
        'resources: 'a,
        M: 'a,
    {
        ready(Ok(WifiControlProgress::Idle))
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
pub struct ConnectedRunner<
    'resources,
    'irq,
    M: RawMutex,
    B,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    irq: &'irq EmbassyMacIrqRuntime<M>,
    network: SplitPinnedRadioRunner<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >,
    services: B,
    rx_backpressured: bool,
}

mod arbitration;
mod owner;
mod service;

#[cfg(test)]
mod tests;
