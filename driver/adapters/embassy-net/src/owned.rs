//! Owned packet boundary used by the Xarxa/Embassy cutover.
//!
//! This module contains no Wi-Fi scheduler. It moves complete [`PacketBuf`]
//! owners between the network and radio execution domains through bounded
//! queues. Peer/TID classification and SRAM promotion happen after the radio
//! side claims a TX owner.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::task::{Context, Poll, Waker};

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Channel, Receiver, Sender, TrySendError};
use embassy_sync::once_lock::OnceLock;
use embassy_sync::signal::Signal;
use embassy_sync::waitqueue::GenericAtomicWaker;
use owned_embassy_net_driver::{Capabilities, Driver, HardwareAddress, LinkState};
use xarxa_driver::{PacketBuf, PacketBufAllocator, PacketPoolWaiter};

use crate::{ETHERNET_HEADER_LEN, FrameLengthError, NetworkInterfaceId, RxEnqueueError};

#[derive(Clone, Copy)]
struct LinkSnapshot {
    epoch: u32,
    up: bool,
}

/// Link lifetime shared by the network producer/consumer pair.
///
/// Bit zero is the level state. The remaining bits form a generation which is
/// advanced on every Down -> Up transition. Queue entries carry that
/// generation, so an owner published concurrently with teardown can never be
/// retargeted to the next association lifetime.
struct OwnedLinkState<M: RawMutex> {
    state: AtomicU32,
    network_waker: GenericAtomicWaker<M>,
    radio_waker: GenericAtomicWaker<M>,
}

impl<M: RawMutex> OwnedLinkState<M> {
    const fn new() -> Self {
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

    fn set(&self, up: bool) {
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

    fn register_network_waker(&self, waker: &Waker) {
        self.network_waker.register(waker);
    }

    fn register_radio_waker(&self, waker: &Waker) {
        self.radio_waker.register(waker);
    }

    fn wake_network(&self) {
        self.network_waker.wake();
    }
}

struct QueuedPacket {
    epoch: u32,
    packet: PacketBuf,
}

/// Static bounded queues for one permanent logical network endpoint.
///
/// Packet bytes do not live in this value. RX owners come from the allocator
/// supplied to [`split`](Self::split); TX owners retain the general Xarxa pool
/// selected by the application.
pub struct OwnedEndpointResources<
    M: RawMutex,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    rx: Channel<M, QueuedPacket, RX_QUEUE_DEPTH>,
    tx: Channel<M, QueuedPacket, TX_QUEUE_DEPTH>,
    tx_published: Signal<M, ()>,
    rx_waiter: OnceLock<PacketPoolWaiter>,
    link: OwnedLinkState<M>,
    split: AtomicBool,
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    OwnedEndpointResources<M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    /// Create an inactive, empty endpoint.
    pub const fn new() -> Self {
        Self {
            rx: Channel::new(),
            tx: Channel::new(),
            tx_published: Signal::new(),
            rx_waiter: OnceLock::new(),
            link: OwnedLinkState::new(),
            split: AtomicBool::new(false),
        }
    }

    /// Split one endpoint into its unique network device and radio owner.
    ///
    /// `rx_allocator` should normally point at an internal-SRAM pool reserved
    /// for frames retained by Xarxa. Its slots return to that pool on the final
    /// [`PacketBuf`] drop, independently of which core performs it.
    pub fn split(
        &mut self,
        interface: NetworkInterfaceId,
        hardware_address: [u8; 6],
        rx_allocator: PacketBufAllocator,
    ) -> (
        OwnedNetworkDevice<'_, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>,
        OwnedNetworkRunner<'_, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>,
    ) {
        assert!(RX_QUEUE_DEPTH != 0, "owned RX queue must not be empty");
        assert!(TX_QUEUE_DEPTH != 0, "owned TX queue must not be empty");
        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "owned endpoint resources may only be split once"
        );
        let resources: &Self = self;
        let rx_waiter = rx_allocator
            .try_claim_waiter()
            .expect("an owned RX pool may have only one asynchronous radio waiter");
        resources
            .rx_waiter
            .init(rx_waiter)
            .unwrap_or_else(|_| unreachable!("owned endpoint initializes its RX waiter once"));
        let rx_waiter = resources
            .rx_waiter
            .try_get()
            .expect("owned RX waiter was initialized");
        (
            OwnedNetworkDevice {
                hardware_address,
                rx: resources.rx.receiver(),
                tx: resources.tx.sender(),
                tx_published: &resources.tx_published,
                link: &resources.link,
            },
            OwnedNetworkRunner {
                interface,
                rx: resources.rx.sender(),
                tx: resources.tx.receiver(),
                tx_published: &resources.tx_published,
                link: &resources.link,
                rx_allocator,
                rx_waiter,
            },
        )
    }
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize> Default
    for OwnedEndpointResources<M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Xarxa/Embassy side of one owned packet endpoint.
pub struct OwnedNetworkDevice<
    'resources,
    M: RawMutex,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    hardware_address: [u8; 6],
    rx: Receiver<'resources, M, QueuedPacket, RX_QUEUE_DEPTH>,
    tx: Sender<'resources, M, QueuedPacket, TX_QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    link: &'resources OwnedLinkState<M>,
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    OwnedNetworkDevice<'_, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    /// Ethernet address reported to the network stack.
    pub const fn hardware_address(&self) -> [u8; 6] {
        self.hardware_address
    }

    /// Register the network runner's one level-state waker.
    pub fn register_waker(&mut self, waker: &Waker) {
        self.link.register_network_waker(waker);
    }

    /// Whether this interface currently belongs to an active role lifetime.
    pub fn link_is_up(&self) -> bool {
        self.link.snapshot().up
    }

    /// Claim the next received packet from the current role lifetime.
    pub fn receive(&mut self) -> Option<PacketBuf> {
        loop {
            let queued = self.rx.try_receive().ok()?;
            let current = self.link.snapshot();
            if current.up && current.epoch == queued.epoch {
                return Some(queued.packet);
            }
            // A stale owner returns to its originating RX pool here.
        }
    }

    /// Whether the bounded software TX ingress can accept one packet now.
    pub fn can_transmit(&self) -> bool {
        self.link.snapshot().up && !self.tx.is_full()
    }

    /// Transfer one complete Ethernet packet into the radio-owned queue.
    ///
    /// Failure returns the unchanged owner. With the unique mutable device
    /// reference, a `true` [`can_transmit`](Self::can_transmit) result followed
    /// immediately by this call cannot fail because of another producer. A
    /// concurrent link-down does not revoke that admission: the packet is
    /// accepted with the observed epoch and is terminally dropped by the radio
    /// consumer instead of violating the driver contract.
    pub fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        let snapshot = self.link.snapshot();
        match self.tx.try_send(QueuedPacket {
            epoch: snapshot.epoch,
            packet,
        }) {
            Ok(()) => {
                self.tx_published.signal(());
                Ok(())
            }
            Err(TrySendError::Full(queued)) => Err(queued.packet),
        }
    }
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize> Driver
    for OwnedNetworkDevice<'_, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    fn capabilities(&self) -> Capabilities {
        Capabilities::default()
    }

    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ethernet(self.hardware_address)
    }

    fn link_state(&mut self) -> LinkState {
        if self.link_is_up() {
            LinkState::Up
        } else {
            LinkState::Down
        }
    }

    fn register_waker(&mut self, waker: &Waker) {
        OwnedNetworkDevice::register_waker(self, waker);
    }

    fn receive(&mut self) -> Option<PacketBuf> {
        OwnedNetworkDevice::receive(self)
    }

    fn can_transmit(&mut self) -> bool {
        OwnedNetworkDevice::can_transmit(self)
    }

    fn transmit(&mut self, packet: PacketBuf) -> Result<(), PacketBuf> {
        OwnedNetworkDevice::transmit(self, packet)
    }
}

/// One driver-owned TX packet claimed by the physical radio.
pub struct OwnedNetworkTxFrame {
    interface: NetworkInterfaceId,
    packet: PacketBuf,
}

/// Copyable RX-only capability for a physical datapath service.
///
/// The capability can allocate only from this endpoint's dedicated RX pool;
/// it cannot observe or claim network-originated TX owners.
#[derive(Clone, Copy)]
pub struct OwnedRxPublisher<'resources, M: RawMutex, const RX_QUEUE_DEPTH: usize> {
    rx: Sender<'resources, M, QueuedPacket, RX_QUEUE_DEPTH>,
    link: &'resources OwnedLinkState<M>,
    rx_allocator: PacketBufAllocator,
    rx_waiter: &'resources PacketPoolWaiter,
}

impl<M: RawMutex, const RX_QUEUE_DEPTH: usize> OwnedRxPublisher<'_, M, RX_QUEUE_DEPTH> {
    /// Number of complete RX owners waiting for Xarxa.
    pub fn queue_len(&self) -> usize {
        self.rx.len()
    }

    /// Poll until one bounded RX queue entry can be published.
    pub fn poll_ready(&self, context: &mut Context<'_>) -> Poll<()> {
        self.link.register_radio_waker(context.waker());
        if !self.link.snapshot().up {
            return Poll::Pending;
        }
        if self.rx.poll_ready_to_send(context).is_pending() {
            return Poll::Pending;
        }
        if self.rx_allocator.has_available() {
            return Poll::Ready(());
        }
        self.rx_waiter.register(context.waker());
        if self.rx_allocator.has_available() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }

    /// Copy one received Ethernet frame into its final Xarxa owner.
    pub fn try_send(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        if frame.len() < ETHERNET_HEADER_LEN {
            return Err(RxEnqueueError::InvalidLength(FrameLengthError::TooShort));
        }
        let mut packet = self
            .rx_allocator
            .try_alloc()
            .ok_or(RxEnqueueError::PoolExhausted)?;
        if frame.len() > packet.capacity() {
            return Err(RxEnqueueError::InvalidLength(FrameLengthError::TooLong));
        }
        packet.set_len(frame.len());
        packet.copy_from_slice(frame);
        self.try_publish(packet)
    }

    /// Build Ethernet-II header and payload directly in the final RX owner.
    pub fn try_send_parts(
        &self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
    ) -> Result<(), RxEnqueueError> {
        let frame_len = ETHERNET_HEADER_LEN
            .checked_add(payload.len())
            .ok_or(RxEnqueueError::InvalidLength(FrameLengthError::TooLong))?;
        let mut packet = self
            .rx_allocator
            .try_alloc()
            .ok_or(RxEnqueueError::PoolExhausted)?;
        if frame_len > packet.capacity() {
            return Err(RxEnqueueError::InvalidLength(FrameLengthError::TooLong));
        }
        packet.set_len(frame_len);
        packet[..6].copy_from_slice(&destination);
        packet[6..12].copy_from_slice(&source);
        packet[12..ETHERNET_HEADER_LEN].copy_from_slice(&ether_type.to_be_bytes());
        packet[ETHERNET_HEADER_LEN..].copy_from_slice(payload);
        self.try_publish(packet)
    }

    /// Publish an already-owned RX packet without copying it again.
    pub fn try_publish(&self, packet: PacketBuf) -> Result<(), RxEnqueueError> {
        if packet.len() < ETHERNET_HEADER_LEN {
            return Err(RxEnqueueError::InvalidLength(FrameLengthError::TooShort));
        }
        let snapshot = self.link.snapshot();
        if !snapshot.up {
            return Err(RxEnqueueError::LinkDown);
        }
        match self.rx.try_send(QueuedPacket {
            epoch: snapshot.epoch,
            packet,
        }) {
            Ok(()) => {
                self.link.wake_network();
                Ok(())
            }
            Err(TrySendError::Full(_)) => Err(RxEnqueueError::QueueFull),
        }
    }
}

impl OwnedNetworkTxFrame {
    /// Logical VIF which accepted this owner.
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    pub(crate) const fn tag(&self) -> &NetworkInterfaceId {
        &self.interface
    }

    /// Complete Ethernet-II bytes.
    pub fn ethernet(&self) -> &[u8] {
        &self.packet
    }

    /// Release the wrapper while retaining the exact packet owner.
    pub fn into_packet(self) -> PacketBuf {
        self.packet
    }
}

/// Link-state capability which can coexist with packet publication handles.
pub struct OwnedLinkController<'resources, M: RawMutex> {
    interface: NetworkInterfaceId,
    link: &'resources OwnedLinkState<M>,
}

impl<M: RawMutex> Clone for OwnedLinkController<'_, M> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex> Copy for OwnedLinkController<'_, M> {}

impl<M: RawMutex> OwnedLinkController<'_, M> {
    /// Permanent logical interface controlled by this capability.
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    /// Publish the role's link level. A Down -> Up edge creates a new epoch.
    pub fn set_link_up(&self, up: bool) {
        self.link.set(up);
    }
}

/// Sole radio-side owner of one network endpoint.
pub struct OwnedNetworkRunner<
    'resources,
    M: RawMutex,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    interface: NetworkInterfaceId,
    rx: Sender<'resources, M, QueuedPacket, RX_QUEUE_DEPTH>,
    tx: Receiver<'resources, M, QueuedPacket, TX_QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    link: &'resources OwnedLinkState<M>,
    rx_allocator: PacketBufAllocator,
    rx_waiter: &'resources PacketPoolWaiter,
}

impl<'resources, M: RawMutex, const RX_QUEUE_DEPTH: usize, const TX_QUEUE_DEPTH: usize>
    OwnedNetworkRunner<'resources, M, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    /// A copyable link-only capability for role lifecycle code.
    pub const fn link_controller(&self) -> OwnedLinkController<'resources, M> {
        OwnedLinkController {
            interface: self.interface,
            link: self.link,
        }
    }

    /// Permanent logical interface represented by this endpoint.
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    /// RX-only publication capability for the physical datapath.
    pub const fn rx_publisher(&self) -> OwnedRxPublisher<'resources, M, RX_QUEUE_DEPTH> {
        OwnedRxPublisher {
            rx: self.rx,
            link: self.link,
            rx_allocator: self.rx_allocator,
            rx_waiter: self.rx_waiter,
        }
    }

    /// Number of complete TX owners waiting before peer/TID classification.
    pub fn tx_queue_len(&self) -> usize {
        self.tx.len()
    }

    /// Copy one received Ethernet frame directly into its final Xarxa owner.
    pub fn try_send_rx(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.rx_publisher().try_send(frame)
    }

    /// Publish an already-owned RX packet without copying it again.
    pub fn try_publish_rx(&self, packet: PacketBuf) -> Result<(), RxEnqueueError> {
        self.rx_publisher().try_publish(packet)
    }

    /// Claim the next current-lifetime TX owner, dropping stale lifetimes.
    pub fn try_receive_tx(&self) -> Option<OwnedNetworkTxFrame> {
        loop {
            let was_full = self.tx.is_full();
            let queued = self.tx.try_receive().ok()?;
            if was_full {
                self.link.wake_network();
            }
            let current = self.link.snapshot();
            if current.up && current.epoch == queued.epoch {
                return Some(OwnedNetworkTxFrame {
                    interface: self.interface,
                    packet: queued.packet,
                });
            }
            // Stale queued data is terminally dropped and returns to its
            // general packet pool before looking at the next owner.
        }
    }

    /// Wait for and claim the next current-lifetime TX owner.
    pub async fn receive_tx(&self) -> OwnedNetworkTxFrame {
        loop {
            if let Some(frame) = self.try_receive_tx() {
                return frame;
            }
            self.tx.ready_to_receive().await;
        }
    }

    /// Wait until some TX publication exists.
    ///
    /// This deliberately has no "wait for BA-sized queue" variant: sparse
    /// work must become eligible immediately. Aggregation policy belongs to
    /// the radio scheduler after it claims the owner.
    pub async fn wait_tx_publication(&self) {
        if self.tx.is_empty() {
            self.tx_published.wait().await;
        }
    }

    /// Wait until the submission frontier reaches `minimum` packets.
    ///
    /// This observes publication events rather than repeatedly polling a
    /// nonempty channel, so a burst collector cannot spin while waiting for
    /// another frame. Callers must still apply their own sparse-traffic
    /// deadline; this method does not require a BA-sized burst.
    pub async fn wait_tx_queue_len_at_least(&self, minimum: usize) {
        while self.tx.len() < minimum {
            self.tx_published.wait().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PinnedNetworkTxFrame, PinnedTxPool, PinnedTxResources};
    use embassy_sync::blocking_mutex::raw::NoopRawMutex;
    use xarxa_driver::{PacketPool, PacketPoolStorage};

    extern crate std;
    use self::std::boxed::Box;
    use self::std::sync::Arc;
    use self::std::sync::atomic::{AtomicUsize, Ordering as StdOrdering};
    use self::std::task::Wake;

    #[derive(Default)]
    struct WakeCount(AtomicUsize);

    impl Wake for WakeCount {
        fn wake(self: Arc<Self>) {
            self.0.fetch_add(1, StdOrdering::Relaxed);
        }

        fn wake_by_ref(self: &Arc<Self>) {
            self.0.fetch_add(1, StdOrdering::Relaxed);
        }
    }

    fn allocator<const N: usize>() -> PacketBufAllocator {
        let storage = Box::leak(Box::new(PacketPoolStorage::<N>::new()));
        Box::leak(Box::new(PacketPool::new(storage))).allocator()
    }

    fn frame(allocator: PacketBufAllocator, marker: u8) -> PacketBuf {
        let mut packet = allocator.try_alloc().unwrap();
        packet.set_len(ETHERNET_HEADER_LEN);
        packet.fill(marker);
        packet
    }

    #[test]
    fn tx_owner_returns_to_its_origin_after_radio_completion() {
        let general = allocator::<1>();
        let rx = allocator::<1>();
        let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
        let (mut device, radio) =
            resources.split(NetworkInterfaceId::new(3), [2, 0, 0, 0, 0, 3], rx);
        radio.link_controller().set_link_up(true);

        device.transmit(frame(general, 0x51)).unwrap();
        assert!(general.try_alloc().is_none());
        let queued = radio.try_receive_tx().unwrap();
        assert_eq!(queued.interface(), NetworkInterfaceId::new(3));
        assert_eq!(queued.ethernet(), &[0x51; ETHERNET_HEADER_LEN]);
        drop(queued);
        assert!(general.try_alloc().is_some());
    }

    #[test]
    fn rx_owner_survives_handoff_and_returns_to_rx_pool() {
        let rx = allocator::<1>();
        let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
        let (mut device, radio) =
            resources.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
        radio.link_controller().set_link_up(true);

        radio
            .rx_publisher()
            .try_send_parts([0x72; 6], [0x73; 6], 0x0800, &[0x74; ETHERNET_HEADER_LEN])
            .unwrap();
        assert!(rx.try_alloc().is_none());
        let packet = device.receive().unwrap();
        assert_eq!(&packet[..6], &[0x72; 6]);
        assert_eq!(&packet[6..12], &[0x73; 6]);
        assert_eq!(&packet[12..14], &[0x08, 0x00]);
        assert_eq!(&packet[14..], &[0x74; ETHERNET_HEADER_LEN]);
        drop(packet);
        assert!(rx.try_alloc().is_some());
    }

    #[test]
    fn rx_readiness_waits_for_both_link_and_pool_capacity() {
        let rx = allocator::<1>();
        let held = rx.try_alloc().unwrap();
        let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
        let (_device, radio) = resources.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
        let link = radio.link_controller();
        let publisher = radio.rx_publisher();
        let wake_count = Arc::new(WakeCount::default());
        let waker = Waker::from(wake_count.clone());
        let mut context = Context::from_waker(&waker);

        assert_eq!(publisher.poll_ready(&mut context), Poll::Pending);
        link.set_link_up(true);
        let after_link = wake_count.0.load(StdOrdering::Relaxed);
        assert_ne!(after_link, 0);
        assert_eq!(publisher.poll_ready(&mut context), Poll::Pending);

        drop(held);
        assert!(wake_count.0.load(StdOrdering::Relaxed) > after_link);
        assert_eq!(publisher.poll_ready(&mut context), Poll::Ready(()));
    }

    #[test]
    fn selected_owner_promotes_once_and_no_credit_returns_it_unchanged() {
        const PHYSICAL_CAPACITY: usize = 64;
        const HEADROOM: usize = 16;
        const TRAILER: usize = 8;
        type PhysicalPool = PinnedTxPool<PHYSICAL_CAPACITY, HEADROOM, TRAILER, 1>;
        type PhysicalResources =
            PinnedTxResources<NoopRawMutex, PHYSICAL_CAPACITY, HEADROOM, TRAILER, 1>;

        let general = allocator::<2>();
        let rx = allocator::<1>();
        let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
        let (mut device, radio) =
            endpoint.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
        radio.link_controller().set_link_up(true);

        let physical_resources = Box::leak(Box::new(PhysicalResources::new()));
        let physical_pool = PhysicalPool::pin_static(Box::leak(Box::new(PhysicalPool::new())));
        let (_network_provider, physical) = physical_resources.split(physical_pool);
        let physical = physical.for_interface(NetworkInterfaceId::new(0));

        device.transmit(frame(general, 0x31)).unwrap();
        device.transmit(frame(general, 0x32)).unwrap();
        assert!(general.try_alloc().is_none());

        let first = match physical.try_promote_owned(radio.try_receive_tx().unwrap()) {
            Ok(frame) => frame,
            Err(_) => panic!("the free physical slot accepts the selected owner"),
        };
        assert_eq!(first.as_slice(), &[0x31; ETHERNET_HEADER_LEN]);

        let second = radio.try_receive_tx().unwrap();
        let second = match physical.try_promote_owned(second) {
            Ok(_) => panic!("the sole retained physical slot exhausts promotion"),
            Err(frame) => frame,
        };
        assert_eq!(second.ethernet(), &[0x32; ETHERNET_HEADER_LEN]);

        // Successful promotion released only the first source owner. The
        // failed promotion still owns the second one byte-for-byte.
        let returned_first = general.try_alloc().unwrap();
        assert!(general.try_alloc().is_none());
        drop(returned_first);

        drop(first);
        let second = match physical.try_promote_owned(second) {
            Ok(frame) => frame,
            Err(_) => panic!("terminal completion returned the physical slot"),
        };
        assert_eq!(second.as_slice(), &[0x32; ETHERNET_HEADER_LEN]);
        assert!(general.try_alloc().is_some());
    }

    #[test]
    fn owned_burst_promotion_does_not_move_a_partial_prefix() {
        const PHYSICAL_CAPACITY: usize = 64;
        const HEADROOM: usize = 16;
        const TRAILER: usize = 8;
        type PhysicalPool = PinnedTxPool<PHYSICAL_CAPACITY, HEADROOM, TRAILER, 1>;
        type PhysicalResources =
            PinnedTxResources<NoopRawMutex, PHYSICAL_CAPACITY, HEADROOM, TRAILER, 1>;

        let general = allocator::<2>();
        let rx = allocator::<1>();
        let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 2>::new()));
        let (mut device, radio) =
            endpoint.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
        radio.link_controller().set_link_up(true);
        device.transmit(frame(general, 0x41)).unwrap();
        device.transmit(frame(general, 0x42)).unwrap();

        let physical_resources = Box::leak(Box::new(PhysicalResources::new()));
        let physical_pool = PhysicalPool::pin_static(Box::leak(Box::new(PhysicalPool::new())));
        let (_network_provider, physical) = physical_resources.split(physical_pool);
        let physical = physical.for_interface(NetworkInterfaceId::new(0));
        let mut burst = [
            Some(PinnedNetworkTxFrame::Owned(radio.try_receive_tx().unwrap())),
            Some(PinnedNetworkTxFrame::Owned(radio.try_receive_tx().unwrap())),
        ];

        assert!(!physical.try_promote_batch(&mut burst));
        assert!(matches!(burst[0], Some(PinnedNetworkTxFrame::Owned(_))));
        assert!(matches!(burst[1], Some(PinnedNetworkTxFrame::Owned(_))));
        assert_eq!(
            burst[0].as_ref().unwrap().as_slice(),
            &[0x41; ETHERNET_HEADER_LEN]
        );
        assert_eq!(
            burst[1].as_ref().unwrap().as_slice(),
            &[0x42; ETHERNET_HEADER_LEN]
        );
        assert!(general.try_alloc().is_none());
    }

    #[test]
    fn link_epoch_prevents_stale_tx_retargeting() {
        let general = allocator::<1>();
        let rx = allocator::<1>();
        let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
        let (mut device, radio) =
            resources.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
        let link = radio.link_controller();
        link.set_link_up(true);
        device.transmit(frame(general, 0x33)).unwrap();

        link.set_link_up(false);
        link.set_link_up(true);
        assert!(radio.try_receive_tx().is_none());
        assert!(general.try_alloc().is_some());
    }

    #[test]
    fn link_down_does_not_revoke_a_synchronous_tx_admission() {
        let general = allocator::<1>();
        let rx = allocator::<1>();
        let resources = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
        let (mut device, radio) =
            resources.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
        let link = radio.link_controller();
        link.set_link_up(true);

        assert!(device.can_transmit());
        link.set_link_up(false);
        assert!(device.transmit(frame(general, 0x44)).is_ok());

        // It belongs to the down lifetime and cannot cross the next up edge.
        link.set_link_up(true);
        assert!(radio.try_receive_tx().is_none());
        assert!(general.try_alloc().is_some());
    }

    #[test]
    fn device_constructs_the_owned_embassy_stack() {
        let general = allocator::<8>();
        let rx = allocator::<1>();
        let endpoint = Box::leak(Box::new(OwnedEndpointResources::<NoopRawMutex, 1, 1>::new()));
        let (device, _radio) = endpoint.split(NetworkInterfaceId::new(0), [2, 0, 0, 0, 0, 1], rx);
        let stack_resources = Box::leak(Box::new(owned_embassy_net::StackResources::new()));

        let (_stack, mut runner) = owned_embassy_net::new(
            device,
            owned_embassy_net::Config::default(),
            stack_resources,
            0x1234,
            general,
        );
        runner.set_poll_budget(owned_embassy_net::PollBudget::new(4, 7));
    }
}
