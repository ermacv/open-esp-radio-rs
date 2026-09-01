#![no_std]
#![forbid(unsafe_code)]

//! Safe, bounded ownership boundary between an open radio task and `embassy-net`.
//!
//! The lower radio task owns all DMA descriptors and chip-specific receive
//! objects. It copies a decapsulated Ethernet frame into [`EthernetFrame`]
//! before calling [`RadioRunner::try_send_rx`]. In the other direction,
//! `embassy-net` builds an owned Ethernet frame that the radio task receives
//! from [`RadioRunner::try_receive_tx`].
//!
//! This follows the source-owned network-device ownership boundary:
//! descriptor and vendor-object pointers never escape into the network stack,
//! and both directions apply explicit bounded backpressure.
//!
//! [`PinnedEndpointResources`] and [`PinnedTxResources`] provide the
//! high-throughput alternative. On RX, the
//! protocol adapter copies directly into a permanently located final slot and
//! passes only its index to the network stack. On TX, the network stack writes
//! directly into a separate permanently located slot with caller-selected
//! headroom and trailer space. The radio receives a lease to that same TX
//! slot, so an IEEE 802.11 encoder can replace the prefix without copying
//! payload.

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
use core::sync::atomic::AtomicBool;
use core::{
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll},
};

pub use embassy_net_driver::{Driver, LinkState, TxToken};
#[cfg(feature = "tx-egress-scheduling")]
pub use embassy_net_driver::{
    EgressDemand, EgressDemandId, EgressDemandLevel, EgressDemandUpdate, EgressKey,
};
pub use embassy_sync::blocking_mutex::raw::{NoopRawMutex, RawMutex};
pub use embassy_sync::signal::Signal;

use embassy_net_driver::{Capabilities, HardwareAddress};
use embassy_sync::{
    channel::{Channel, Receiver, Sender, TrySendError},
    waitqueue::GenericAtomicWaker,
};

#[cfg(feature = "tx-egress-scheduling")]
mod egress_control;
#[cfg(feature = "tx-egress-scheduling")]
mod egress_demand;
#[cfg(feature = "tx-phase-telemetry")]
mod egress_grant;
#[cfg(feature = "tx-egress-scheduling")]
mod egress_key;
mod egress_peer;
mod pinned;
#[cfg(feature = "tx-phase-telemetry")]
mod tx_performance;

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
pub use egress_control::EgressControlSnapshot;
#[cfg(all(
    feature = "tx-egress-scheduling",
    feature = "tx-egress-diagnostic-switch"
))]
pub use egress_control::configure_egress_control_for_diagnostics;
#[cfg(feature = "tx-egress-scheduling")]
pub use egress_control::{
    DEFAULT_EGRESS_CONTROL_DEPTH, DEFAULT_EGRESS_NETWORK_SERVICE_BUDGET,
    DEFAULT_EGRESS_RADIO_SERVICE_BUDGET, DefaultDualEgressControlledNetwork,
    DefaultDualEgressRadioOwner, DefaultEgressControlPlane, DefaultEgressControlledNetwork,
    DefaultEgressNetworkPort, DefaultEgressNetworkScheduler, DefaultEgressNetworkState,
    DefaultEgressRadioOwner, DefaultEgressRadioPort, DefaultEgressRadioScheduler,
    DefaultEgressRadioWake, DualEgressRadioOwner, EgressControlPlane, EgressControlledNetwork,
    EgressNetworkPort, EgressNetworkScheduler, EgressNetworkState, EgressRadioControlOwner,
    EgressRadioOwner, EgressRadioPort, EgressRadioScheduler, EgressRadioWake,
    EgressSharedRadioWake,
};
#[cfg(feature = "tx-phase-telemetry")]
pub use egress_grant::{EgressShadowGrant, EgressShadowGrantError, EgressShadowGrantSnapshot};
#[cfg(feature = "tx-egress-scheduling")]
pub use egress_key::{AssociatedEgressIdentity, EgressGrantKey};
pub use egress_peer::{
    EgressPeerDirectory, EgressPeerDirectoryError, EgressPeerIdentity, EgressPeerResolver,
};
#[cfg(feature = "tx-phase-telemetry")]
pub use pinned::PinnedTxOwnershipSnapshot;
#[cfg(feature = "tx-staging-copy-probe")]
pub use pinned::configure_tx_staging_copy_probe;
pub use pinned::{
    DualPinnedNetworkRunner, EgressQueueTopology, NetworkEndpointConfig, NetworkInterfaceId,
    PinnedEndpointResources, PinnedNetworkLinkController, PinnedNetworkRunner,
    PinnedNetworkTxFrame, PinnedReceiveToken, PinnedRxPublisher, PinnedTransmitToken,
    PinnedTxConsumer, PinnedTxFrame, PinnedTxInterfaceConsumer, PinnedTxMetadata, PinnedTxOwnerTag,
    PinnedTxPool, PinnedTxProvider, PinnedTxResources, SharedPinnedReceiveToken,
    SharedPinnedRxConsumer, SharedPinnedRxPublisher, SharedPinnedRxQueue, SharedPoolReceiveToken,
    SharedRxSplitPinnedDevice, SplitPinnedDevice, SplitPinnedRxRunner,
};
#[cfg(feature = "tx-core1-materializer-probe")]
pub use pinned::{
    PINNED_TX_MATERIALIZATION_BATCH_CAPACITY, PinnedTxCore1MaterializationPoll,
    TX_CORE1_MATERIALIZER_COUNTERS, TxCore1MaterializerCounters, TxCore1MaterializerSnapshot,
    configure_tx_core1_materializer_probe,
};
#[cfg(feature = "tx-phase-telemetry")]
pub use tx_performance::{
    TX_PERFORMANCE, TxPerformanceCounters, TxPerformanceSample, TxPerformanceSnapshot,
};

/// Ethernet header length, excluding an FCS.
pub const ETHERNET_HEADER_LEN: usize = 14;

#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
static KEYED_EGRESS_SCHEDULING_ENABLED: AtomicBool = AtomicBool::new(true);
#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
static KEYED_EGRESS_MULTI_DISPATCH_ENABLED: AtomicBool = AtomicBool::new(true);

/// Select ordinary FIFO egress only for a same-image TX scheduler diagnostic.
///
/// Production builds do not expose this switch and always use keyed egress
/// scheduling while the link is up.
#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
pub fn configure_keyed_egress_schedule_for_diagnostics(enabled: bool) {
    KEYED_EGRESS_SCHEDULING_ENABLED.store(enabled, Ordering::Release);
}

/// Select one-packet socket dispatch only for a same-image Xarxa batching
/// control. Production emits a bounded four-packet quantum per socket pass.
#[cfg(all(feature = "tx-egress-scheduling", feature = "tx-phase-telemetry"))]
pub fn configure_keyed_egress_multi_dispatch_for_diagnostics(enabled: bool) {
    KEYED_EGRESS_MULTI_DISPATCH_ENABLED.store(enabled, Ordering::Release);
}

#[cfg(feature = "tx-egress-scheduling")]
pub(crate) fn keyed_egress_scheduling_enabled() -> bool {
    #[cfg(feature = "tx-phase-telemetry")]
    {
        KEYED_EGRESS_SCHEDULING_ENABLED.load(Ordering::Acquire)
    }
    #[cfg(not(feature = "tx-phase-telemetry"))]
    {
        true
    }
}

#[cfg(feature = "tx-egress-scheduling")]
pub(crate) fn keyed_egress_dispatch_quantum() -> u8 {
    #[cfg(feature = "tx-phase-telemetry")]
    {
        if KEYED_EGRESS_MULTI_DISPATCH_ENABLED.load(Ordering::Acquire) {
            4
        } else {
            1
        }
    }
    #[cfg(not(feature = "tx-phase-telemetry"))]
    {
        4
    }
}

/// Breaks an unbounded network-stack ingress drain at the
/// adapter's physical queue boundary.
///
/// The ordinary stack poll drains a device until `receive` returns `None`.
/// With a concurrently refilled radio queue that can fill an application
/// socket and keep dropping later packets before its consumer is polled. The
/// synthetic `None` self-wakes the network task; Embassy can then poll every
/// socket consumer already woken by this epoch before continuing immediately.
pub(crate) struct IngressPollFairness {
    remaining: usize,
}

impl IngressPollFairness {
    pub(crate) const fn new(epoch_capacity: usize) -> Self {
        Self {
            remaining: epoch_capacity,
        }
    }

    pub(crate) fn admit(&mut self, cx: &mut Context<'_>, epoch_capacity: usize) -> bool {
        if self.remaining == 0 {
            self.remaining = epoch_capacity;
            cx.waker().wake_by_ref();
            false
        } else {
            true
        }
    }

    pub(crate) fn record_received(&mut self) {
        self.remaining -= 1;
    }

    pub(crate) fn record_natural_stop(&mut self, epoch_capacity: usize) {
        self.remaining = epoch_capacity;
    }
}

/// A complete Ethernet frame with storage owned by its current pipeline stage.
///
/// `CAPACITY` includes the Ethernet header and excludes the Ethernet FCS.
pub struct EthernetFrame<const CAPACITY: usize> {
    length: usize,
    bytes: [u8; CAPACITY],
}

impl<const CAPACITY: usize> EthernetFrame<CAPACITY> {
    /// Copies one complete Ethernet frame into owned storage.
    pub fn copy_from_slice(frame: &[u8]) -> Result<Self, FrameLengthError> {
        if frame.len() < ETHERNET_HEADER_LEN {
            return Err(FrameLengthError::TooShort);
        }
        if frame.len() > CAPACITY {
            return Err(FrameLengthError::TooLong);
        }

        let mut owned = Self::with_length(frame.len());
        owned.as_mut_slice().copy_from_slice(frame);
        Ok(owned)
    }

    /// Copies one borrowed Ethernet-II header/payload view directly into its
    /// final owned queue allocation.
    pub fn copy_from_parts(
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
    ) -> Result<Self, FrameLengthError> {
        let length = ETHERNET_HEADER_LEN
            .checked_add(payload.len())
            .ok_or(FrameLengthError::TooLong)?;
        if length > CAPACITY {
            return Err(FrameLengthError::TooLong);
        }
        let mut owned = Self::with_length(length);
        owned.bytes[..6].copy_from_slice(&destination);
        owned.bytes[6..12].copy_from_slice(&source);
        owned.bytes[12..14].copy_from_slice(&ether_type.to_be_bytes());
        owned.bytes[14..length].copy_from_slice(payload);
        Ok(owned)
    }

    fn with_length(length: usize) -> Self {
        Self {
            length,
            bytes: [0; CAPACITY],
        }
    }

    /// Returns the populated portion of this frame.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes[..self.length]
    }

    /// Returns the populated portion of this frame mutably.
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.bytes[..self.length]
    }

    /// Returns the complete Ethernet frame length.
    pub fn len(&self) -> usize {
        self.length
    }

    /// Returns whether this frame has no bytes.
    ///
    /// Valid frames created through the public API are never empty.
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
}

/// Why a byte slice cannot be represented by an owned Ethernet frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FrameLengthError {
    /// The slice is shorter than an Ethernet header.
    TooShort,
    /// The slice exceeds the configured frame storage.
    TooLong,
}

/// Why a received frame was not admitted to the network stack.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RxEnqueueError {
    /// The supplied Ethernet frame length is invalid.
    InvalidLength(FrameLengthError),
    /// The fixed receive queue is full.
    QueueFull,
}

pub(crate) struct SharedLinkState<M: RawMutex> {
    // Bit zero is the link state. The upper 31 bits are a wrapping egress
    // lifecycle epoch incremented on every Down -> Up transition. Publishing
    // both in one atomic word prevents the stack from observing a new link
    // with the previous scheduling epoch.
    state: AtomicU32,
    waker: GenericAtomicWaker<M>,
}

impl<M: RawMutex> SharedLinkState<M> {
    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            waker: GenericAtomicWaker::new(M::INIT),
        }
    }

    pub(crate) fn set(&self, state: LinkState) {
        let up = state == LinkState::Up;
        let mut current = self.state.load(Ordering::Acquire);
        loop {
            if (current & 1 != 0) == up {
                return;
            }
            let next = if up {
                current.wrapping_add(2) | 1
            } else {
                current & !1
            };
            match self.state.compare_exchange_weak(
                current,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => {
                    self.waker.wake();
                    return;
                }
                Err(observed) => current = observed,
            }
        }
    }

    pub(crate) fn get(&self, cx: &mut Context<'_>) -> LinkState {
        // Register first, then load: a concurrent change either wakes this
        // waker or is observed by the following acquire load.
        self.waker.register(cx.waker());
        if self.state.load(Ordering::Acquire) & 1 != 0 {
            LinkState::Up
        } else {
            LinkState::Down
        }
    }

    #[cfg(feature = "tx-egress-scheduling")]
    pub(crate) fn is_up(&self) -> bool {
        self.state.load(Ordering::Acquire) & 1 != 0
    }

    #[cfg(feature = "tx-egress-scheduling")]
    pub(crate) fn egress_epoch(&self) -> u32 {
        self.state.load(Ordering::Acquire) >> 1
    }
}

/// Static storage for one `embassy-net` device and its radio-side runner.
///
/// Two queues are allocated inline. Memory use is therefore deterministic:
/// `2 * QUEUE_DEPTH * FRAME_CAPACITY` bytes plus small queue metadata.
pub struct Resources<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> {
    rx: Channel<M, EthernetFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx: Channel<M, EthernetFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    link: SharedLinkState<M>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    Resources<M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    /// Creates empty RX/TX queues with the link down.
    pub const fn new() -> Self {
        Self {
            rx: Channel::new(),
            tx: Channel::new(),
            link: SharedLinkState::new(),
        }
    }

    /// Splits the storage into exclusive network-stack and radio-side handles.
    ///
    /// The returned `Device` is the only TX producer. That exclusivity is
    /// what lets it reserve queue capacity before returning an infallible
    /// `embassy-net` transmit token.
    pub fn split(
        &mut self,
        station_address: [u8; 6],
    ) -> (
        Device<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>,
        RadioRunner<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>,
    ) {
        (
            Device {
                rx: self.rx.receiver(),
                tx: self.tx.sender(),
                link: &self.link,
                station_address,
                tx_reservation: (),
                ingress_fairness: IngressPollFairness::new(QUEUE_DEPTH),
            },
            RadioRunner {
                rx: self.rx.sender(),
                tx: self.tx.receiver(),
                link: &self.link,
            },
        )
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Default
    for Resources<M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// The `embassy-net` side of the ownership boundary.
pub struct Device<'resources, M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> {
    rx: Receiver<'resources, M, EthernetFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx: Sender<'resources, M, EthernetFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    link: &'resources SharedLinkState<M>,
    station_address: [u8; 6],
    // Borrowed by every TX token. The GAT lifetime prevents a second token
    // being requested while the first one is live, preserving its reservation.
    tx_reservation: (),
    ingress_fairness: IngressPollFairness,
}

/// The radio-task side of the ownership boundary.
pub struct RadioRunner<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    rx: Sender<'resources, M, EthernetFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx: Receiver<'resources, M, EthernetFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    link: &'resources SharedLinkState<M>,
}

impl<'resources, M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    RadioRunner<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    /// Updates the link state reported to `embassy-net`.
    pub fn set_link_state(&self, state: LinkState) {
        self.link.set(state);
    }

    /// Copies a decapsulated Ethernet frame into the bounded RX queue.
    pub fn try_send_rx(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        let owned = EthernetFrame::copy_from_slice(frame).map_err(RxEnqueueError::InvalidLength)?;
        self.rx.try_send(owned).map_err(|error| match error {
            TrySendError::Full(_) => RxEnqueueError::QueueFull,
        })
    }

    /// Waits for capacity and copies a decapsulated Ethernet frame into RX.
    pub async fn send_rx(&self, frame: &[u8]) -> Result<(), FrameLengthError> {
        let owned = EthernetFrame::copy_from_slice(frame)?;
        self.rx.send(owned).await;
        Ok(())
    }

    /// Takes the next Ethernet frame that the stack wants transmitted.
    pub fn try_receive_tx(&self) -> Option<EthernetFrame<FRAME_CAPACITY>> {
        self.tx.try_receive().ok()
    }

    /// Waits for the next Ethernet frame that the stack wants transmitted.
    pub async fn receive_tx(&self) -> EthernetFrame<FRAME_CAPACITY> {
        self.tx.receive().await
    }

    /// Number of decapsulated RX frames awaiting `embassy-net`.
    pub fn rx_queue_len(&self) -> usize {
        self.rx.len()
    }

    /// Number of Ethernet TX frames awaiting the radio task.
    pub fn tx_queue_len(&self) -> usize {
        self.tx.len()
    }
}

/// An owned receive token; consuming it releases its fixed queue slot.
pub struct ReceiveToken<const FRAME_CAPACITY: usize> {
    frame: EthernetFrame<FRAME_CAPACITY>,
}

impl<const FRAME_CAPACITY: usize> embassy_net_driver::RxToken for ReceiveToken<FRAME_CAPACITY> {
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(self.frame.as_mut_slice())
    }
}

/// A reserved transmit token.
pub struct TransmitToken<
    'device,
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    tx: Sender<'resources, M, EthernetFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    _reservation: &'device mut (),
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> embassy_net_driver::TxToken
    for TransmitToken<'_, '_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn consume<R, F>(self, length: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        assert!(
            length <= FRAME_CAPACITY,
            "embassy-net requested a frame larger than driver capabilities"
        );

        let mut frame = EthernetFrame::with_length(length);
        let result = f(frame.as_mut_slice());

        // Device::transmit polls capacity before constructing this token.
        // The token's mutable borrow prevents another token from being issued,
        // and this private Sender is the queue's only producer.
        if let Err(TrySendError::Full(_)) = self.tx.try_send(frame) {
            unreachable!("reserved open-radio TX queue slot was lost");
        }
        result
    }
}

impl<'resources, M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Driver
    for Device<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    type RxToken<'device>
        = ReceiveToken<FRAME_CAPACITY>
    where
        Self: 'device;
    type TxToken<'device>
        = TransmitToken<'device, 'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>
    where
        Self: 'device;

    fn receive(&mut self, cx: &mut Context<'_>) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.ingress_fairness.admit(cx, QUEUE_DEPTH) {
            return None;
        }
        // A receive token must be accompanied by a guaranteed TX token.
        if self.tx.poll_ready_to_send(cx).is_pending() {
            self.ingress_fairness.record_natural_stop(QUEUE_DEPTH);
            return None;
        }

        let frame = match self.rx.poll_receive(cx) {
            Poll::Ready(frame) => frame,
            Poll::Pending => {
                self.ingress_fairness.record_natural_stop(QUEUE_DEPTH);
                return None;
            }
        };
        self.ingress_fairness.record_received();

        Some((
            ReceiveToken { frame },
            TransmitToken {
                tx: self.tx,
                _reservation: &mut self.tx_reservation,
            },
        ))
    }

    fn transmit(&mut self, cx: &mut Context<'_>) -> Option<Self::TxToken<'_>> {
        self.tx
            .poll_ready_to_send(cx)
            .is_ready()
            .then_some(TransmitToken {
                tx: self.tx,
                _reservation: &mut self.tx_reservation,
            })
    }

    fn link_state(&mut self, cx: &mut Context<'_>) -> LinkState {
        self.link.get(cx)
    }

    fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::default();
        capabilities.max_transmission_unit = FRAME_CAPACITY;
        capabilities.max_burst_size = Some(QUEUE_DEPTH);
        capabilities
    }

    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ethernet(self.station_address)
    }
}
