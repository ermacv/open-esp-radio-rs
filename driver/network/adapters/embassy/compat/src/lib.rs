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
    sync::atomic::{AtomicU32, Ordering},
    task::{Context, Poll},
};

pub use embassy_net_driver::{Driver, RxToken, TxToken};
pub use embassy_sync::blocking_mutex::raw::{NoopRawMutex, RawMutex};
pub use embassy_sync::signal::Signal;
pub use open_esp_radio_network::{
    ETHERNET_HEADER_LEN, FrameLengthError, LinkState, NetworkInterfaceId, RxEnqueueError,
};

use embassy_net_driver::{
    Capabilities, ChecksumCapabilities, HardwareAddress, LinkState as DriverLinkState,
};
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
    const fn empty() -> Self {
        Self {
            length: 0,
            bytes: [0; CAPACITY],
        }
    }

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

    fn copy_from_slice_in_place(&mut self, frame: &[u8]) -> Result<(), FrameLengthError> {
        if frame.len() < ETHERNET_HEADER_LEN {
            return Err(FrameLengthError::TooShort);
        }
        if frame.len() > CAPACITY {
            return Err(FrameLengthError::TooLong);
        }
        self.length = frame.len();
        self.as_mut_slice().copy_from_slice(frame);
        Ok(())
    }

    fn copy_from_parts_in_place(
        &mut self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
    ) -> Result<(), FrameLengthError> {
        let length = ETHERNET_HEADER_LEN
            .checked_add(payload.len())
            .ok_or(FrameLengthError::TooLong)?;
        if length > CAPACITY {
            return Err(FrameLengthError::TooLong);
        }
        self.length = length;
        self.bytes[..6].copy_from_slice(&destination);
        self.bytes[6..12].copy_from_slice(&source);
        self.bytes[12..14].copy_from_slice(&ether_type.to_be_bytes());
        self.bytes[14..length].copy_from_slice(payload);
        Ok(())
    }

    fn prepare(&mut self, length: usize) -> &mut [u8] {
        assert!(
            length <= CAPACITY,
            "embassy-net requested a frame larger than driver capabilities"
        );
        self.length = length;
        self.as_mut_slice()
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

/// Payload-only backing for one direction of a compatibility endpoint.
///
/// The storage is intentionally separate from [`Resources`]. A board may put
/// these bytes in general memory while keeping queue indexes, wakers and link
/// atomics in low-latency internal SRAM. Once split, every slot circulates as
/// one unique mutable lease; frame bytes are never moved through a channel.
pub struct FrameStorage<const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> {
    frames: [EthernetFrame<FRAME_CAPACITY>; QUEUE_DEPTH],
}

impl<const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    FrameStorage<FRAME_CAPACITY, QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            frames: [const { EthernetFrame::empty() }; QUEUE_DEPTH],
        }
    }
}

impl<const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Default
    for FrameStorage<FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
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

type FrameSlot<const FRAME_CAPACITY: usize> = &'static mut EthernetFrame<FRAME_CAPACITY>;

struct QueuedFrame<const FRAME_CAPACITY: usize> {
    epoch: u32,
    frame: FrameSlot<FRAME_CAPACITY>,
}

/// Hot queue/link metadata for one unchanged `embassy-net` endpoint.
///
/// Payload bytes live in two separately supplied [`FrameStorage`] arenas.
/// These channels move only unique frame leases, never complete frame values.
pub struct Resources<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> {
    rx_queue_full: AtomicU32,
    rx_free: Channel<M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
    rx_ready: Channel<M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_free: Channel<M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_ready: Channel<M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_published: Signal<M, ()>,
    link: SharedLinkState<M>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    Resources<M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    /// Creates empty ownership queues with the link down.
    pub const fn new() -> Self {
        Self {
            rx_queue_full: AtomicU32::new(0),
            rx_free: Channel::new(),
            rx_ready: Channel::new(),
            tx_free: Channel::new(),
            tx_ready: Channel::new(),
            tx_published: Signal::new(),
            link: SharedLinkState::new(),
        }
    }

    /// Binds payload arenas and splits exclusive stack/radio capabilities.
    ///
    /// Storage must be static because frame owners can cross executor and core
    /// boundaries. Every slot enters exactly one free queue here and returns
    /// to it only after the corresponding token/lease reaches terminal drop.
    pub fn split(
        &'static mut self,
        station_address: [u8; 6],
        rx_storage: &'static mut FrameStorage<FRAME_CAPACITY, QUEUE_DEPTH>,
        tx_storage: &'static mut FrameStorage<FRAME_CAPACITY, QUEUE_DEPTH>,
    ) -> (
        Device<'static, M, FRAME_CAPACITY, QUEUE_DEPTH>,
        RadioRunner<'static, M, FRAME_CAPACITY, QUEUE_DEPTH>,
    ) {
        assert!(
            FRAME_CAPACITY >= ETHERNET_HEADER_LEN,
            "compatibility frame capacity must hold an Ethernet header"
        );
        assert!(QUEUE_DEPTH != 0, "compatibility queues must not be empty");
        for frame in &mut rx_storage.frames {
            assert!(
                self.rx_free.try_send(frame).is_ok(),
                "every RX payload slot must enter the free queue exactly once"
            );
        }
        for frame in &mut tx_storage.frames {
            assert!(
                self.tx_free.try_send(frame).is_ok(),
                "every TX payload slot must enter the free queue exactly once"
            );
        }
        (
            Device {
                rx_ready: self.rx_ready.receiver(),
                rx_free: self.rx_free.sender(),
                tx_ready: self.tx_ready.sender(),
                tx_free: self.tx_free.receiver(),
                tx_free_return: self.tx_free.sender(),
                tx_published: &self.tx_published,
                link: &self.link,
                station_address,
                checksum: ChecksumCapabilities::default(),
                tx_reservation: (),
                ingress_fairness: IngressPollFairness::new(QUEUE_DEPTH),
            },
            RadioRunner {
                rx_queue_full: &self.rx_queue_full,
                rx_ready: self.rx_ready.sender(),
                rx_free: self.rx_free.receiver(),
                rx_free_return: self.rx_free.sender(),
                tx_ready: self.tx_ready.receiver(),
                tx_free: self.tx_free.sender(),
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

/// The unchanged `embassy-net-driver` side of the ownership boundary.
pub struct Device<'resources, M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> {
    rx_ready: Receiver<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    rx_free: Sender<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_ready: Sender<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_free: Receiver<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_free_return: Sender<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    link: &'resources SharedLinkState<M>,
    station_address: [u8; 6],
    checksum: ChecksumCapabilities,
    tx_reservation: (),
    ingress_fairness: IngressPollFairness,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    Device<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    /// Override checksum work advertised to an unchanged `embassy-net` stack.
    pub fn with_checksum_capabilities(mut self, checksum: ChecksumCapabilities) -> Self {
        self.checksum = checksum;
        self
    }
}

/// The radio-task side of the ownership boundary.
pub struct RadioRunner<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    rx_queue_full: &'resources AtomicU32,
    rx_ready: Sender<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    rx_free: Receiver<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
    rx_free_return: Sender<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_ready: Receiver<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_free: Sender<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    link: &'resources SharedLinkState<M>,
}

/// Read-only queue observation without borrowing the radio executor.
pub struct ResourceMonitor<'a, M: RawMutex, const F: usize, const Q: usize> {
    rx_queue_full: &'a AtomicU32,
    rx_ready: Sender<'a, M, QueuedFrame<F>, Q>,
    rx_free: Receiver<'a, M, FrameSlot<F>, Q>,
    tx_ready: Receiver<'a, M, QueuedFrame<F>, Q>,
    tx_free: Sender<'a, M, FrameSlot<F>, Q>,
}

/// Queue lengths are sampled individually; a live producer may advance between
/// reads. Tokens held by the stack or radio are absent from both queues.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ResourceSnapshot {
    /// Cumulative publication refusals due to occupied RX storage, wrapping
    /// at u32::MAX. A refusal counts once per attempt, not once per packet.
    pub rx_queue_full: u32,
    pub rx_ready: usize,
    pub rx_free: usize,
    pub tx_ready: usize,
    pub tx_free: usize,
}

impl<M: RawMutex, const F: usize, const Q: usize> ResourceMonitor<'_, M, F, Q> {
    pub fn snapshot(&self) -> ResourceSnapshot {
        ResourceSnapshot {
            rx_queue_full: self.rx_queue_full.load(Ordering::Relaxed),
            rx_ready: self.rx_ready.len(),
            rx_free: self.rx_free.len(),
            tx_ready: self.tx_ready.len(),
            tx_free: self.tx_free.len(),
        }
    }
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
    rx_queue_full: &'resources AtomicU32,
    ready: Sender<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    free: Receiver<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
    free_return: Sender<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
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
        self.ready.len()
    }

    pub fn poll_ready(&self, context: &mut Context<'_>) -> Poll<()> {
        self.link.register_radio_waker(context.waker());
        if !self.link.snapshot().up {
            return Poll::Pending;
        }
        self.free.poll_ready_to_receive(context)
    }

    pub fn try_send(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        let snapshot = self.link.snapshot();
        if !snapshot.up {
            return Err(RxEnqueueError::LinkDown);
        }
        let owned = self.free.try_receive().map_err(|_| {
            self.rx_queue_full.fetch_add(1, Ordering::Relaxed);
            RxEnqueueError::QueueFull
        })?;
        if let Err(error) = owned.copy_from_slice_in_place(frame) {
            self.release(owned);
            return Err(RxEnqueueError::InvalidLength(error));
        }
        self.publish(snapshot.epoch, owned)
    }

    pub fn try_send_parts(
        &self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
    ) -> Result<(), RxEnqueueError> {
        let snapshot = self.link.snapshot();
        if !snapshot.up {
            return Err(RxEnqueueError::LinkDown);
        }
        let owned = self.free.try_receive().map_err(|_| {
            self.rx_queue_full.fetch_add(1, Ordering::Relaxed);
            RxEnqueueError::QueueFull
        })?;
        if let Err(error) = owned.copy_from_parts_in_place(destination, source, ether_type, payload)
        {
            self.release(owned);
            return Err(RxEnqueueError::InvalidLength(error));
        }
        self.publish(snapshot.epoch, owned)
    }

    fn publish(&self, epoch: u32, frame: FrameSlot<FRAME_CAPACITY>) -> Result<(), RxEnqueueError> {
        if let Err(TrySendError::Full(queued)) = self.ready.try_send(QueuedFrame { epoch, frame }) {
            self.release(queued.frame);
            unreachable!("an acquired RX payload slot must have one ready-queue credit");
        }
        Ok(())
    }

    fn release(&self, frame: FrameSlot<FRAME_CAPACITY>) {
        if let Err(TrySendError::Full(_)) = self.free_return.try_send(frame) {
            unreachable!("an RX payload lease may return to the free queue only once");
        }
    }
}

/// Unique complete-frame owner returned by the upstream-compatible TX queue.
pub struct RadioTxFrame<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    frame: Option<FrameSlot<FRAME_CAPACITY>>,
    free: Sender<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    RadioTxFrame<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    pub fn as_slice(&self) -> &[u8] {
        self.frame
            .as_deref()
            .expect("live TX lease always owns its payload slot")
            .as_slice()
    }

    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    pub fn is_empty(&self) -> bool {
        self.as_slice().is_empty()
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Drop
    for RadioTxFrame<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(frame) = self.frame.take()
            && let Err(TrySendError::Full(_)) = self.free.try_send(frame)
        {
            unreachable!("a TX payload lease may return to the free queue only once");
        }
    }
}

/// Copyable consumer of complete upstream-compatible TX frames.
pub struct RadioTxConsumer<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    ready: Receiver<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    free: Sender<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
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

impl<'resources, M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    RadioTxConsumer<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    pub fn queue_len(&self) -> usize {
        self.ready.len()
    }

    pub fn try_receive(&self) -> Option<RadioTxFrame<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>> {
        loop {
            let queued = self.ready.try_receive().ok()?;
            let current = self.link.snapshot();
            if current.up && current.epoch == queued.epoch {
                return Some(RadioTxFrame {
                    frame: Some(queued.frame),
                    free: self.free,
                });
            }
            if let Err(TrySendError::Full(_)) = self.free.try_send(queued.frame) {
                unreachable!("discarding stale TX must return exactly one payload slot");
            }
        }
    }

    pub async fn receive(&self) -> RadioTxFrame<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH> {
        loop {
            if let Some(frame) = self.try_receive() {
                return frame;
            }
            self.ready.ready_to_receive().await;
        }
    }

    pub async fn wait_for_publication(self) {
        if self.ready.is_empty() {
            self.tx_published.wait().await;
        }
    }

    pub async fn wait_for_queue_len_at_least(self, minimum: usize) {
        while self.ready.len() < minimum {
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

    pub const fn resource_monitor(
        &self,
    ) -> ResourceMonitor<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH> {
        ResourceMonitor {
            rx_queue_full: self.rx_queue_full,
            rx_ready: self.rx_ready,
            rx_free: self.rx_free,
            tx_ready: self.tx_ready,
            tx_free: self.tx_free,
        }
    }

    pub const fn rx_publisher(
        &self,
    ) -> RadioRxPublisher<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH> {
        RadioRxPublisher {
            rx_queue_full: self.rx_queue_full,
            ready: self.rx_ready,
            free: self.rx_free,
            free_return: self.rx_free_return,
            link: self.link,
        }
    }

    pub const fn tx_consumer(&self) -> RadioTxConsumer<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH> {
        RadioTxConsumer {
            ready: self.tx_ready,
            free: self.tx_free,
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
    pub fn try_receive_tx(
        &self,
    ) -> Option<RadioTxFrame<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>> {
        self.tx_consumer().try_receive()
    }

    /// Waits for the next Ethernet frame that the stack wants transmitted.
    pub async fn receive_tx(&self) -> RadioTxFrame<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH> {
        self.tx_consumer().receive().await
    }

    /// Number of decapsulated RX frames awaiting `embassy-net`.
    pub fn rx_queue_len(&self) -> usize {
        self.rx_ready.len()
    }

    /// Number of Ethernet TX frames awaiting the radio task.
    pub fn tx_queue_len(&self) -> usize {
        self.tx_consumer().queue_len()
    }
}

/// An owned receive token; consuming it releases its fixed queue slot.
pub struct ReceiveToken<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    frame: Option<FrameSlot<FRAME_CAPACITY>>,
    free: Sender<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> embassy_net_driver::RxToken
    for ReceiveToken<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        f(self
            .frame
            .as_deref_mut()
            .expect("live RX token always owns its payload slot")
            .as_mut_slice())
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Drop
    for ReceiveToken<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(frame) = self.frame.take()
            && let Err(TrySendError::Full(_)) = self.free.try_send(frame)
        {
            unreachable!("an RX token may return its payload slot only once");
        }
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
    frame: Option<FrameSlot<FRAME_CAPACITY>>,
    ready: Sender<'resources, M, QueuedFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    free: Sender<'resources, M, FrameSlot<FRAME_CAPACITY>, QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    epoch: u32,
    _reservation: &'device mut (),
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> embassy_net_driver::TxToken
    for TransmitToken<'_, '_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn consume<R, F>(mut self, length: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let result = f(self
            .frame
            .as_deref_mut()
            .expect("live TX token always owns its reserved payload slot")
            .prepare(length));
        let frame = self
            .frame
            .take()
            .expect("completed TX token still owns its payload slot");
        if let Err(TrySendError::Full(queued)) = self.ready.try_send(QueuedFrame {
            epoch: self.epoch,
            frame,
        }) {
            if let Err(TrySendError::Full(_)) = self.free.try_send(queued.frame) {
                unreachable!("failed TX publication must restore its exact payload slot");
            }
            unreachable!("reserved open-radio TX queue slot was lost");
        }
        self.tx_published.signal(());
        result
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Drop
    for TransmitToken<'_, '_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(frame) = self.frame.take()
            && let Err(TrySendError::Full(_)) = self.free.try_send(frame)
        {
            unreachable!("an unused TX token may restore its payload slot only once");
        }
    }
}

impl<'resources, M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Driver
    for Device<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    type RxToken<'device>
        = ReceiveToken<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>
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
        if self.tx_free.poll_ready_to_receive(cx).is_pending() {
            self.ingress_fairness.record_natural_stop(QUEUE_DEPTH);
            return None;
        }

        let (frame, frame_epoch) = loop {
            match self.rx_ready.poll_receive(cx) {
                Poll::Ready(queued) => {
                    let current = self.link.snapshot();
                    if current.up && current.epoch == queued.epoch {
                        break (queued.frame, current.epoch);
                    }
                    if let Err(TrySendError::Full(_)) = self.rx_free.try_send(queued.frame) {
                        unreachable!("discarding stale RX must restore exactly one payload slot");
                    }
                }
                Poll::Pending => {
                    self.ingress_fairness.record_natural_stop(QUEUE_DEPTH);
                    return None;
                }
            }
        };
        let tx_frame = self
            .tx_free
            .try_receive()
            .expect("single device owner preserves the preflight TX payload credit");
        self.ingress_fairness.record_received();

        Some((
            ReceiveToken {
                frame: Some(frame),
                free: self.rx_free,
            },
            TransmitToken {
                frame: Some(tx_frame),
                ready: self.tx_ready,
                free: self.tx_free_return,
                tx_published: self.tx_published,
                // Use the lifetime which accepted this RX frame rather than
                // the earlier preflight snapshot. A concurrent Down -> Up may
                // advance the epoch while stale queue entries are drained.
                epoch: frame_epoch,
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
        let frame = match self.tx_free.poll_receive(cx) {
            Poll::Ready(frame) => frame,
            Poll::Pending => return None,
        };
        Some(TransmitToken {
            frame: Some(frame),
            ready: self.tx_ready,
            free: self.tx_free_return,
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
        capabilities.checksum = self.checksum.clone();
        capabilities
    }

    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ethernet(self.station_address)
    }
}
