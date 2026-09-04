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
//! This crate is deliberately limited to the unchanged upstream Embassy driver
//! contract. Optimized owned-packet and physical Wi-Fi execution adapters live
//! in separate crates and cannot enter this dependency graph through features.

use core::{
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    task::{Context, Poll},
};

pub use embassy_net_driver::{Driver, RxToken, TxToken};
pub use embassy_sync::blocking_mutex::raw::{NoopRawMutex, RawMutex};
pub use embassy_sync::signal::Signal;
pub use open_esp_radio_network::{
    ETHERNET_HEADER_LEN, FrameLengthError, LinkState, NetworkInterfaceId, RxEnqueueError,
};

use embassy_net_driver::{Capabilities, HardwareAddress, LinkState as DriverLinkState};
use embassy_sync::{
    channel::{Channel, Receiver, Sender, TrySendError},
    waitqueue::GenericAtomicWaker,
};

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

#[derive(Clone, Copy)]
struct LinkSnapshot {
    epoch: u32,
    up: bool,
}

pub(crate) struct SharedLinkState<M: RawMutex> {
    // Bit zero is the link state. The upper 31 bits are a wrapping egress
    // lifecycle epoch incremented on every Down -> Up transition. Publishing
    // both in one atomic word prevents the stack from observing a new link
    // with the previous scheduling epoch.
    state: AtomicU32,
    network_waker: GenericAtomicWaker<M>,
    radio_waker: GenericAtomicWaker<M>,
}

impl<M: RawMutex> SharedLinkState<M> {
    pub(crate) const fn new() -> Self {
        Self {
            state: AtomicU32::new(0),
            network_waker: GenericAtomicWaker::new(M::INIT),
            radio_waker: GenericAtomicWaker::new(M::INIT),
        }
    }

    fn snapshot(&self) -> LinkSnapshot {
        let state = self.state.load(Ordering::Acquire);
        LinkSnapshot {
            epoch: state >> 1,
            up: state & 1 != 0,
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
                    self.network_waker.wake();
                    self.radio_waker.wake();
                    return;
                }
                Err(observed) => current = observed,
            }
        }
    }

    pub(crate) fn get(&self, cx: &mut Context<'_>) -> DriverLinkState {
        // Register first, then load: a concurrent change either wakes this
        // waker or is observed by the following acquire load.
        self.network_waker.register(cx.waker());
        if self.state.load(Ordering::Acquire) & 1 != 0 {
            DriverLinkState::Up
        } else {
            DriverLinkState::Down
        }
    }

    fn register_radio_waker(&self, waker: &core::task::Waker) {
        self.radio_waker.register(waker);
    }
}

struct QueuedFrame<const CAPACITY: usize> {
    epoch: u32,
    frame: EthernetFrame<CAPACITY>,
}

/// Static storage for one `embassy-net` device and its radio-side runner.
///
/// Two queues are allocated inline. Memory use is therefore deterministic:
/// `2 * QUEUE_DEPTH * FRAME_CAPACITY` bytes plus small queue metadata.
pub struct Resources<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> {
    rx: Channel<M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx: Channel<M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_published: Signal<M, ()>,
    link: SharedLinkState<M>,
    split: AtomicBool,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    Resources<M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    /// Creates empty RX/TX queues with the link down.
    pub const fn new() -> Self {
        Self {
            rx: Channel::new(),
            tx: Channel::new(),
            tx_published: Signal::new(),
            link: SharedLinkState::new(),
            split: AtomicBool::new(false),
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
        assert!(
            FRAME_CAPACITY >= ETHERNET_HEADER_LEN,
            "compatibility frame capacity must hold an Ethernet header"
        );
        assert!(QUEUE_DEPTH != 0, "compatibility queues must not be empty");
        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "compatibility endpoint resources may only be split once"
        );
        (
            Device {
                rx: self.rx.receiver(),
                tx: self.tx.sender(),
                tx_published: &self.tx_published,
                link: &self.link,
                station_address,
                tx_reservation: (),
                ingress_fairness: IngressPollFairness::new(QUEUE_DEPTH),
            },
            RadioRunner {
                rx: self.rx.sender(),
                tx: self.tx.receiver(),
                tx_published: &self.tx_published,
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
    rx: Receiver<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx: Sender<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
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
    rx: Sender<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx: Receiver<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    link: &'resources SharedLinkState<M>,
}

/// Copyable link-only authority retained by a finite radio role.
pub struct RadioLinkController<'resources, M: RawMutex> {
    link: &'resources SharedLinkState<M>,
}

impl<M: RawMutex> Clone for RadioLinkController<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for RadioLinkController<'_, M> {}

impl<M: RawMutex> RadioLinkController<'_, M> {
    pub fn set_link_state(&self, state: LinkState) {
        self.link.set(state);
    }
}

/// Copyable RX-only compatibility capability for the physical datapath.
pub struct RadioRxPublisher<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    rx: Sender<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    link: &'resources SharedLinkState<M>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Clone
    for RadioRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Copy
    for RadioRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    RadioRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    pub fn queue_len(&self) -> usize {
        self.rx.len()
    }

    pub fn poll_ready(&self, context: &mut Context<'_>) -> Poll<()> {
        self.link.register_radio_waker(context.waker());
        if !self.link.snapshot().up {
            return Poll::Pending;
        }
        self.rx.poll_ready_to_send(context)
    }

    pub fn try_send(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        let owned = EthernetFrame::copy_from_slice(frame).map_err(RxEnqueueError::InvalidLength)?;
        self.try_publish(owned)
    }

    pub fn try_send_parts(
        &self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
    ) -> Result<(), RxEnqueueError> {
        let owned = EthernetFrame::copy_from_parts(destination, source, ether_type, payload)
            .map_err(RxEnqueueError::InvalidLength)?;
        self.try_publish(owned)
    }

    fn try_publish(&self, frame: EthernetFrame<FRAME_CAPACITY>) -> Result<(), RxEnqueueError> {
        let snapshot = self.link.snapshot();
        if !snapshot.up {
            return Err(RxEnqueueError::LinkDown);
        }
        self.rx
            .try_send(QueuedFrame {
                epoch: snapshot.epoch,
                frame,
            })
            .map_err(|error| match error {
                TrySendError::Full(_) => RxEnqueueError::QueueFull,
            })
    }
}

/// Copyable consumer of complete upstream-compatible TX frames.
pub struct RadioTxConsumer<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    tx: Receiver<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    link: &'resources SharedLinkState<M>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Clone
    for RadioTxConsumer<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Copy
    for RadioTxConsumer<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    RadioTxConsumer<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    pub fn queue_len(&self) -> usize {
        self.tx.len()
    }

    pub fn try_receive(&self) -> Option<EthernetFrame<FRAME_CAPACITY>> {
        loop {
            let queued = self.tx.try_receive().ok()?;
            let current = self.link.snapshot();
            if current.up && current.epoch == queued.epoch {
                return Some(queued.frame);
            }
        }
    }

    pub async fn receive(&self) -> EthernetFrame<FRAME_CAPACITY> {
        loop {
            if let Some(frame) = self.try_receive() {
                return frame;
            }
            self.tx.ready_to_receive().await;
        }
    }

    pub async fn wait_for_publication(self) {
        if self.tx.is_empty() {
            self.tx_published.wait().await;
        }
    }

    pub async fn wait_for_queue_len_at_least(self, minimum: usize) {
        while self.tx.len() < minimum {
            self.tx_published.wait().await;
        }
    }
}

impl<'resources, M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    RadioRunner<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    pub const fn link_controller(&self) -> RadioLinkController<'resources, M> {
        RadioLinkController { link: self.link }
    }

    pub const fn rx_publisher(
        &self,
    ) -> RadioRxPublisher<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH> {
        RadioRxPublisher {
            rx: self.rx,
            link: self.link,
        }
    }

    pub const fn tx_consumer(&self) -> RadioTxConsumer<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH> {
        RadioTxConsumer {
            tx: self.tx,
            tx_published: self.tx_published,
            link: self.link,
        }
    }

    /// Updates the link state reported to `embassy-net`.
    pub fn set_link_state(&self, state: LinkState) {
        self.link_controller().set_link_state(state);
    }

    /// Copies a decapsulated Ethernet frame into the bounded RX queue.
    pub fn try_send_rx(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.rx_publisher().try_send(frame)
    }

    /// Takes the next Ethernet frame that the stack wants transmitted.
    pub fn try_receive_tx(&self) -> Option<EthernetFrame<FRAME_CAPACITY>> {
        self.tx_consumer().try_receive()
    }

    /// Waits for the next Ethernet frame that the stack wants transmitted.
    pub async fn receive_tx(&self) -> EthernetFrame<FRAME_CAPACITY> {
        self.tx_consumer().receive().await
    }

    /// Number of decapsulated RX frames awaiting `embassy-net`.
    pub fn rx_queue_len(&self) -> usize {
        self.rx.len()
    }

    /// Number of Ethernet TX frames awaiting the radio task.
    pub fn tx_queue_len(&self) -> usize {
        self.tx_consumer().queue_len()
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
    tx: Sender<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    epoch: u32,
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
        if let Err(TrySendError::Full(_)) = self.tx.try_send(QueuedFrame {
            epoch: self.epoch,
            frame,
        }) {
            unreachable!("reserved open-radio TX queue slot was lost");
        }
        self.tx_published.signal(());
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
        let link = self.link.snapshot();
        if !link.up {
            let _ = self.link.get(cx);
            self.ingress_fairness.record_natural_stop(QUEUE_DEPTH);
            return None;
        }
        // A receive token must be accompanied by a guaranteed TX token.
        if self.tx.poll_ready_to_send(cx).is_pending() {
            self.ingress_fairness.record_natural_stop(QUEUE_DEPTH);
            return None;
        }

        let frame = loop {
            match self.rx.poll_receive(cx) {
                Poll::Ready(queued) => {
                    let current = self.link.snapshot();
                    if current.up && current.epoch == queued.epoch {
                        break queued.frame;
                    }
                }
                Poll::Pending => {
                    self.ingress_fairness.record_natural_stop(QUEUE_DEPTH);
                    return None;
                }
            }
        };
        self.ingress_fairness.record_received();

        Some((
            ReceiveToken { frame },
            TransmitToken {
                tx: self.tx,
                tx_published: self.tx_published,
                epoch: link.epoch,
                _reservation: &mut self.tx_reservation,
            },
        ))
    }

    fn transmit(&mut self, cx: &mut Context<'_>) -> Option<Self::TxToken<'_>> {
        let link = self.link.snapshot();
        if !link.up {
            let _ = self.link.get(cx);
            return None;
        }
        self.tx
            .poll_ready_to_send(cx)
            .is_ready()
            .then_some(TransmitToken {
                tx: self.tx,
                tx_published: self.tx_published,
                epoch: link.epoch,
                _reservation: &mut self.tx_reservation,
            })
    }

    fn link_state(&mut self, cx: &mut Context<'_>) -> DriverLinkState {
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
