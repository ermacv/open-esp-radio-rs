//! Permanently located RX/TX slots for bounded, copy-minimal network ownership.

use core::{
    cell::Cell,
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

use embassy_net_driver::{Capabilities, Driver, HardwareAddress, LinkState};
use embassy_sync::{
    blocking_mutex::Mutex,
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender, TryReceiveError, TrySendError},
    signal::Signal,
};
use open_esp_radio_dma::{
    DmaIndexReturn, PinnedDmaTxNetworkLease, PinnedDmaTxPool, PinnedDmaTxRadioLease,
    ReturningStableDmaBacking, RxHandoffPool, RxNetworkLease, RxRadioLease,
};

use crate::{ETHERNET_HEADER_LEN, FrameLengthError, RxEnqueueError, SharedLinkState};

/// Role-selected Ethernet identity shared by the persistent network device
/// and the currently active radio epoch.
///
/// The address is changed only while the link is down. Keeping it beside the
/// queues allows one `embassy-net` device to survive sequential STA and AP
/// epochs without manufacturing a second network owner or retaining the STA
/// address in AP mode.
struct SharedHardwareAddress<M: RawMutex> {
    address: Mutex<M, Cell<[u8; 6]>>,
}

/// One publication frontier shared by copied and externally-backed RX slots.
///
/// The physical pools remain separate, but `embassy-net` observes this typed
/// stream in exact publication order. Its capacity covers the complete S31
/// production geometry (64 owned plus 32 shared slots) without relying on a
/// priority rule between two independent queues.
const ORDERED_RX_READY_CAPACITY: usize = 96;
const ORDERED_RX_SHARED_BIT: u8 = 1 << 7;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum OrderedRxSource {
    Owned(u8),
    Shared(u8),
}

/// Compact typed encoding for the common ready frontier. Production pool
/// indices are below 128, leaving the high bit as an unambiguous source tag
/// while keeping each channel record one byte wide.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct OrderedRxReady(u8);

const _: [(); 1] = [(); core::mem::size_of::<OrderedRxReady>()];

impl OrderedRxReady {
    fn owned(index: u8) -> Self {
        assert!(
            index < ORDERED_RX_SHARED_BIT,
            "ordered owned RX index must fit in seven bits"
        );
        Self(index)
    }

    fn shared(index: u8) -> Self {
        assert!(
            index < ORDERED_RX_SHARED_BIT,
            "ordered shared RX index must fit in seven bits"
        );
        Self(index | ORDERED_RX_SHARED_BIT)
    }

    const fn source(self) -> OrderedRxSource {
        if self.0 & ORDERED_RX_SHARED_BIT == 0 {
            OrderedRxSource::Owned(self.0)
        } else {
            OrderedRxSource::Shared(self.0 & !ORDERED_RX_SHARED_BIT)
        }
    }
}

impl<M: RawMutex> SharedHardwareAddress<M> {
    const fn new(address: [u8; 6]) -> Self {
        Self {
            address: Mutex::new(Cell::new(address)),
        }
    }

    fn set(&self, address: [u8; 6]) {
        self.address.lock(|current| current.set(address));
    }

    fn get(&self) -> [u8; 6] {
        self.address.lock(Cell::get)
    }
}

/// Ready-index channel for an RX pool owned by a lower staging layer.
///
/// The bytes remain in that external [`RxHandoffPool`]. This resource stores
/// only the ownership publication edge consumed by the network device.
pub struct SharedPinnedRxQueue<M: RawMutex, const SLOT_COUNT: usize> {
    ready: Channel<M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>,
    split: AtomicBool,
}

impl<M: RawMutex, const SLOT_COUNT: usize> SharedPinnedRxQueue<M, SLOT_COUNT> {
    pub const fn new() -> Self {
        Self {
            ready: Channel::new(),
            split: AtomicBool::new(false),
        }
    }

    pub fn split<'resources, const FRAME_CAPACITY: usize>(
        &'resources self,
        pool: &'resources RxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>,
        on_release: fn(),
    ) -> (
        SharedPinnedRxPublisher<'resources, M, SLOT_COUNT>,
        SharedPinnedRxConsumer<'resources, M, FRAME_CAPACITY, SLOT_COUNT>,
    ) {
        assert!(SLOT_COUNT > 0, "shared pinned RX pool must not be empty");
        assert!(
            SLOT_COUNT <= usize::from(ORDERED_RX_SHARED_BIT),
            "shared pinned RX index must fit in seven bits"
        );
        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "shared pinned RX queue may only be split once"
        );
        (
            SharedPinnedRxPublisher {
                ready: self.ready.sender(),
            },
            SharedPinnedRxConsumer {
                ready: self.ready.receiver(),
                ready_sender: self.ready.sender(),
                pool,
                on_release,
            },
        )
    }

    /// Recreate the cheap producer endpoint after the unique consumer has
    /// been installed. Sequential radio epochs may each own one such handle.
    pub fn publisher(&self) -> SharedPinnedRxPublisher<'_, M, SLOT_COUNT> {
        assert!(
            self.split.load(Ordering::Acquire),
            "shared pinned RX queue must be split before publication"
        );
        SharedPinnedRxPublisher {
            ready: self.ready.sender(),
        }
    }
}

impl<M: RawMutex, const SLOT_COUNT: usize> Default for SharedPinnedRxQueue<M, SLOT_COUNT> {
    fn default() -> Self {
        Self::new()
    }
}

/// Protocol-side capability to publish one already formatted external slot.
pub struct SharedPinnedRxPublisher<'resources, M: RawMutex, const SLOT_COUNT: usize> {
    ready: Sender<'resources, M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>,
}

impl<M: RawMutex, const SLOT_COUNT: usize> Clone for SharedPinnedRxPublisher<'_, M, SLOT_COUNT> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const SLOT_COUNT: usize> Copy for SharedPinnedRxPublisher<'_, M, SLOT_COUNT> {}

impl<M: RawMutex, const SLOT_COUNT: usize> SharedPinnedRxPublisher<'_, M, SLOT_COUNT> {
    #[inline(always)]
    pub fn publish(&self, index: u8) {
        if let Err(TrySendError::Full(_)) = self.ready.try_send(OrderedRxReady::shared(index)) {
            unreachable!("ordered RX frontier covers every owned and shared slot");
        }
    }

    pub fn queue_len(&self) -> usize {
        self.ready.len()
    }
}

pub struct SharedPinnedRxConsumer<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const SLOT_COUNT: usize,
> {
    ready: Receiver<'resources, M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>,
    ready_sender: Sender<'resources, M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>,
    pool: &'resources RxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>,
    on_release: fn(),
}

/// Static resources for copy-minimal RX and copy-free TX ownership boundaries.
///
/// RX is copied once from the protocol adapter directly into its final slot;
/// only a slot index crosses the queue. [`PinnedTxPool`] owns the separate
/// DMA-visible TX slots. `embassy-net` sees only each TX slot's middle Ethernet
/// region; the radio lease sees the complete allocation and remains its unique
/// owner until dropped. The TX pool must be pinned before [`Self::split`].
///
/// SOURCE: complete `libnet80211.a[ieee80211_output.o]::
/// ieee80211_alloc_tx_buf` cache-TX/type-nine path and complete
/// `libpp.a[esf_buf.o]::{esf_buf_setup,esf_buf_alloc}`.
pub struct SplitPinnedResources<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    free_rx: Channel<M, u8, RX_QUEUE_DEPTH>,
    ready_rx: Channel<M, u8, RX_QUEUE_DEPTH>,
    rx_pool: RxHandoffPool<FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    free_tx: Channel<M, u8, TX_QUEUE_DEPTH>,
    ready_tx: Channel<M, u8, TX_QUEUE_DEPTH>,
    tx_published: Signal<M, ()>,
    link: SharedLinkState<M>,
    hardware_address: SharedHardwareAddress<M>,
    split: AtomicBool,
}

/// Permanently located storage for the TX allocations exposed to radio DMA.
///
/// This is separate from [`SplitPinnedResources`] so a platform linker can place
/// only the DMA-visible bytes in internal SRAM while keeping RX queues and
/// Embassy synchronization state in ordinary memory.
pub type PinnedTxPool<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = PinnedDmaTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>;

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> SplitPinnedResources<M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            free_rx: Channel::new(),
            ready_rx: Channel::new(),
            rx_pool: RxHandoffPool::new(),
            free_tx: Channel::new(),
            ready_tx: Channel::new(),
            tx_published: Signal::new(),
            link: SharedLinkState::new(),
            hardware_address: SharedHardwareAddress::new([0; 6]),
            split: AtomicBool::new(false),
        }
    }

    pub fn split<'resources>(
        &'resources mut self,
        pool: Pin<&'resources mut PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>,
        station_address: [u8; 6],
    ) -> (
        SplitPinnedDevice<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
        SplitPinnedRadioRunner<
            'resources,
            M,
            FRAME_CAPACITY,
            HEADROOM,
            TRAILER,
            RX_QUEUE_DEPTH,
            TX_QUEUE_DEPTH,
        >,
    ) {
        assert!(RX_QUEUE_DEPTH > 0, "pinned RX pool must not be empty");
        assert!(TX_QUEUE_DEPTH > 0, "pinned TX pool must not be empty");
        assert!(
            RX_QUEUE_DEPTH <= usize::from(u8::MAX) + 1,
            "pinned RX pool index must fit in u8"
        );
        assert!(
            TX_QUEUE_DEPTH <= usize::from(u8::MAX) + 1,
            "pinned TX pool index must fit in u8"
        );

        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "pinned resources may only be split once"
        );
        for index in 0..RX_QUEUE_DEPTH {
            self.free_rx
                .try_send(index as u8)
                .expect("an empty free RX queue accepts every pool index");
        }
        for index in 0..TX_QUEUE_DEPTH {
            self.free_tx
                .try_send(index as u8)
                .expect("an empty free queue accepts every pool index");
        }
        let pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH> =
            Pin::into_ref(pool).get_ref();
        let resources: &Self = self;
        resources.hardware_address.set(station_address);

        (
            SplitPinnedDevice {
                ready_rx: resources.ready_rx.receiver(),
                free_rx: resources.free_rx.sender(),
                rx_pool: &resources.rx_pool,
                free_tx: resources.free_tx.receiver(),
                free_tx_return: resources.free_tx.sender(),
                ready_tx: resources.ready_tx.sender(),
                tx_published: &resources.tx_published,
                tx_pool: pool,
                link: &resources.link,
                hardware_address: &resources.hardware_address,
                ingress_tx: None,
                application_tx: None,
                reserve_ingress_tx: false,
                tx_reservation: (),
            },
            SplitPinnedRadioRunner {
                free_rx: resources.free_rx.receiver(),
                free_rx_return: resources.free_rx.sender(),
                ready_rx: resources.ready_rx.sender(),
                ordered_rx: None,
                rx_pool: &resources.rx_pool,
                free_tx: resources.free_tx.sender(),
                ready_tx: resources.ready_tx.receiver(),
                ready_tx_return: resources.ready_tx.sender(),
                tx_published: &resources.tx_published,
                tx_pool: pool,
                link: &resources.link,
                hardware_address: &resources.hardware_address,
            },
        )
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> Default
    for SplitPinnedResources<M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

pub struct SplitPinnedDevice<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    ready_rx: Receiver<'resources, M, u8, RX_QUEUE_DEPTH>,
    free_rx: Sender<'resources, M, u8, RX_QUEUE_DEPTH>,
    rx_pool: &'resources RxHandoffPool<FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    free_tx: Receiver<'resources, M, u8, TX_QUEUE_DEPTH>,
    free_tx_return: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    ready_tx: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    link: &'resources SharedLinkState<M>,
    hardware_address: &'resources SharedHardwareAddress<M>,
    /// One credit unavailable to ordinary egress and therefore available to
    /// satisfy the `Driver::receive` RX+TX-token contract under saturated TX.
    ingress_tx: Option<u8>,
    application_tx: Option<u8>,
    reserve_ingress_tx: bool,
    tx_reservation: (),
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    SplitPinnedDevice<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    /// Keep one TX credit unavailable to ordinary application egress so an
    /// incoming frame can always receive the paired `TxToken` required by the
    /// embassy-net driver contract. Resource profiles enabling this must add
    /// one credit beyond their advertised application capacity.
    pub fn with_ingress_tx_reserve(mut self) -> Self {
        assert!(
            TX_QUEUE_DEPTH > 1,
            "ingress TX reserve needs an application credit"
        );
        self.reserve_ingress_tx = true;
        self
    }

    fn poll_reserve_ingress_tx(&mut self, cx: &mut Context<'_>) -> bool {
        if self.ingress_tx.is_none()
            && let Poll::Ready(index) = self.free_tx.poll_receive(cx)
        {
            self.ingress_tx = Some(index);
        }
        self.ingress_tx.is_some()
    }

    fn poll_reserve_application_tx(&mut self, cx: &mut Context<'_>) -> bool {
        if self.application_tx.is_some() {
            return true;
        }
        // Move the last free credit out of the application-visible queue. A
        // later RX poll can consume it even if application egress stays full.
        if self.reserve_ingress_tx && self.ingress_tx.is_none() && self.free_tx.len() <= 1 {
            let _ = self.poll_reserve_ingress_tx(cx);
        }
        if let Poll::Ready(index) = self.free_tx.poll_receive(cx) {
            self.application_tx = Some(index);
        }
        self.application_tx.is_some()
    }

    fn take_tx_token<'device>(
        &'device mut self,
        index: u8,
    ) -> PinnedTransmitToken<
        'device,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    > {
        let lease = self.tx_pool.claim_network(index);
        PinnedTransmitToken {
            free_tx: self.free_tx_return,
            ready_tx: self.ready_tx,
            tx_published: self.tx_published,
            lease: Some(lease),
            _reservation: &mut self.tx_reservation,
        }
    }

    /// Add a second RX source whose storage remains owned by a lower staging
    /// pool. Ordinary frames can then cross into `embassy-net` by index while
    /// this device's original RX pool remains available for copying slow
    /// paths such as A-MSDU expansion.
    pub fn with_shared_rx<const SHARED_CAPACITY: usize, const SHARED_SLOTS: usize>(
        self,
        shared: SharedPinnedRxConsumer<'resources, M, SHARED_CAPACITY, SHARED_SLOTS>,
    ) -> SharedRxSplitPinnedDevice<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
        SHARED_CAPACITY,
        SHARED_SLOTS,
    > {
        SharedRxSplitPinnedDevice {
            inner: self,
            shared,
        }
    }
}

/// Unique `embassy-net` lease for one permanently located received frame.
///
/// Consuming or dropping the token returns the slot to the radio publisher.
/// The frame bytes therefore stay at one stable address across the
/// radio-to-network ownership handoff.
pub struct PinnedReceiveToken<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    free_rx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    lease: Option<RxNetworkLease<'resources, FRAME_CAPACITY>>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> embassy_net_driver::RxToken
    for PinnedReceiveToken<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        self.lease
            .as_mut()
            .expect("live pinned RX token")
            .with_frame(f)
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Drop
    for PinnedReceiveToken<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let index = lease.release();
            if let Err(TrySendError::Full(_)) = self.free_rx.try_send(index) {
                unreachable!("network RX token returns its unique pinned index");
            }
        }
    }
}

/// Network token backed by a lower staging pool rather than the adapter's
/// copying slow-path pool.
pub struct SharedPoolReceiveToken<'resources, const FRAME_CAPACITY: usize> {
    lease: Option<RxNetworkLease<'resources, FRAME_CAPACITY>>,
    on_release: fn(),
}

impl<const FRAME_CAPACITY: usize> embassy_net_driver::RxToken
    for SharedPoolReceiveToken<'_, FRAME_CAPACITY>
{
    fn consume<R, F>(mut self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        self.lease
            .as_mut()
            .expect("live shared RX token")
            .with_frame(f)
    }
}

impl<const FRAME_CAPACITY: usize> Drop for SharedPoolReceiveToken<'_, FRAME_CAPACITY> {
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            drop(lease);
            (self.on_release)();
        }
    }
}

/// RX token for a device that accepts both copied slow-path frames and
/// in-place frames retained in the lower staging pool.
pub enum SharedPinnedReceiveToken<
    'resources,
    M: RawMutex,
    const OWNED_CAPACITY: usize,
    const OWNED_DEPTH: usize,
    const SHARED_CAPACITY: usize,
> {
    Owned(PinnedReceiveToken<'resources, M, OWNED_CAPACITY, OWNED_DEPTH>),
    Shared(SharedPoolReceiveToken<'resources, SHARED_CAPACITY>),
}

impl<
    M: RawMutex,
    const OWNED_CAPACITY: usize,
    const OWNED_DEPTH: usize,
    const SHARED_CAPACITY: usize,
> embassy_net_driver::RxToken
    for SharedPinnedReceiveToken<'_, M, OWNED_CAPACITY, OWNED_DEPTH, SHARED_CAPACITY>
{
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        match self {
            Self::Owned(token) => embassy_net_driver::RxToken::consume(token, f),
            Self::Shared(token) => embassy_net_driver::RxToken::consume(token, f),
        }
    }
}

/// `embassy-net` device that multiplexes an in-place staging pool and the
/// adapter-owned copying pool while retaining one common TX/link owner.
pub struct SharedRxSplitPinnedDevice<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
    const SHARED_CAPACITY: usize,
    const SHARED_SLOTS: usize,
> {
    inner: SplitPinnedDevice<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >,
    shared: SharedPinnedRxConsumer<'resources, M, SHARED_CAPACITY, SHARED_SLOTS>,
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> Drop
    for SplitPinnedDevice<'_, M, FRAME_CAPACITY, HEADROOM, TRAILER, RX_QUEUE_DEPTH, TX_QUEUE_DEPTH>
{
    fn drop(&mut self) {
        for index in [self.ingress_tx.take(), self.application_tx.take()]
            .into_iter()
            .flatten()
        {
            if let Err(TrySendError::Full(_)) = self.free_tx_return.try_send(index) {
                unreachable!("reserved pinned TX index was lost");
            }
        }
    }
}

pub struct PinnedTransmitToken<
    'device,
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    free_tx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    ready_tx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    lease: Option<PinnedDmaTxNetworkLease<'resources, FRAME_CAPACITY, HEADROOM, TRAILER>>,
    _reservation: &'device mut (),
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> embassy_net_driver::TxToken
    for PinnedTransmitToken<'_, '_, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn consume<R, F>(mut self, length: usize, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        assert!(
            length <= FRAME_CAPACITY,
            "embassy-net requested a frame larger than pinned driver capabilities"
        );
        let lease = self.lease.take().expect("TX token consumed once");
        let (index, result) = lease.publish(length, f);
        if let Err(TrySendError::Full(_)) = self.ready_tx.try_send(index) {
            unreachable!("one ready entry exists per non-free pinned TX slot");
        }
        self.tx_published.signal(());
        result
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> Drop for PinnedTransmitToken<'_, '_, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(lease) = self.lease.take() {
            let index = lease.release();
            if let Err(TrySendError::Full(_)) = self.free_tx.try_send(index) {
                unreachable!("dropped pinned TX token returns its unique index");
            }
        }
    }
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> Driver
    for SplitPinnedDevice<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    type RxToken<'device>
        = PinnedReceiveToken<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
    where
        Self: 'device;
    type TxToken<'device>
        = PinnedTransmitToken<
        'device,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    >
    where
        Self: 'device;

    fn receive(&mut self, cx: &mut Context<'_>) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.poll_reserve_ingress_tx(cx) {
            return None;
        }
        let index = match self.ready_rx.poll_receive(cx) {
            Poll::Ready(index) => index,
            Poll::Pending => return None,
        };
        let lease = self.rx_pool.claim_network(index);
        let tx_index = self
            .ingress_tx
            .take()
            .expect("ingress admission reserves one TX credit");
        Some((
            PinnedReceiveToken {
                free_rx: self.free_rx,
                lease: Some(lease),
            },
            self.take_tx_token(tx_index),
        ))
    }

    fn transmit(&mut self, cx: &mut Context<'_>) -> Option<Self::TxToken<'_>> {
        if !self.poll_reserve_application_tx(cx) {
            return None;
        }
        let index = self
            .application_tx
            .take()
            .expect("application admission reserves one TX credit");
        Some(self.take_tx_token(index))
    }

    fn link_state(&mut self, cx: &mut Context<'_>) -> LinkState {
        self.link.get(cx)
    }

    fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::default();
        capabilities.max_transmission_unit = FRAME_CAPACITY;
        capabilities.max_burst_size = Some(RX_QUEUE_DEPTH.min(TX_QUEUE_DEPTH));
        capabilities
    }

    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ethernet(self.hardware_address.get())
    }
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
    const SHARED_CAPACITY: usize,
    const SHARED_SLOTS: usize,
> Driver
    for SharedRxSplitPinnedDevice<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
        SHARED_CAPACITY,
        SHARED_SLOTS,
    >
{
    type RxToken<'device>
        = SharedPinnedReceiveToken<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH, SHARED_CAPACITY>
    where
        Self: 'device;
    type TxToken<'device>
        = PinnedTransmitToken<
        'device,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    >
    where
        Self: 'device;

    fn receive(&mut self, cx: &mut Context<'_>) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.inner.poll_reserve_ingress_tx(cx) {
            return None;
        }
        let ready = match self.shared.ready.poll_receive(cx) {
            Poll::Ready(ready) => ready,
            Poll::Pending => return None,
        };
        let rx = match ready.source() {
            OrderedRxSource::Owned(index) => {
                let lease = self.inner.rx_pool.claim_network(index);
                SharedPinnedReceiveToken::Owned(PinnedReceiveToken {
                    free_rx: self.inner.free_rx,
                    lease: Some(lease),
                })
            }
            OrderedRxSource::Shared(index) => {
                let lease = self.shared.pool.claim_network(index);
                SharedPinnedReceiveToken::Shared(SharedPoolReceiveToken {
                    lease: Some(lease),
                    on_release: self.shared.on_release,
                })
            }
        };
        let tx_index = self
            .inner
            .ingress_tx
            .take()
            .expect("ordered ingress admission reserves one TX credit");
        let tx = self.inner.take_tx_token(tx_index);
        Some((rx, tx))
    }

    fn transmit(&mut self, cx: &mut Context<'_>) -> Option<Self::TxToken<'_>> {
        self.inner.transmit(cx)
    }

    fn link_state(&mut self, cx: &mut Context<'_>) -> LinkState {
        self.inner.link_state(cx)
    }

    fn capabilities(&self) -> Capabilities {
        let mut capabilities = self.inner.capabilities();
        capabilities.max_burst_size = Some(
            RX_QUEUE_DEPTH
                .saturating_add(SHARED_SLOTS)
                .min(TX_QUEUE_DEPTH),
        );
        capabilities
    }

    fn hardware_address(&self) -> HardwareAddress {
        self.inner.hardware_address()
    }
}

/// Narrow radio-side capability that can only publish received Ethernet
/// frames to `embassy-net`.
///
/// This view deliberately contains no TX capability. It can therefore be
/// moved into an RX protocol sink while [`SplitPinnedRadioRunner`] remains the
/// unique owner of TX leases.
pub struct PinnedRxPublisher<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const QUEUE_DEPTH: usize,
> {
    free_rx: Receiver<'resources, M, u8, QUEUE_DEPTH>,
    free_rx_return: Sender<'resources, M, u8, QUEUE_DEPTH>,
    ready_rx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    ordered_rx: Option<Sender<'resources, M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>>,
    rx_pool: &'resources RxHandoffPool<FRAME_CAPACITY, QUEUE_DEPTH>,
    reserved_rx: Option<u8>,
}

impl<'resources, M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    PinnedRxPublisher<'resources, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn validate_length(length: usize) -> Result<(), FrameLengthError> {
        if length < ETHERNET_HEADER_LEN {
            Err(FrameLengthError::TooShort)
        } else if length > FRAME_CAPACITY {
            Err(FrameLengthError::TooLong)
        } else {
            Ok(())
        }
    }

    fn try_claim_slot(
        &mut self,
    ) -> Result<RxRadioLease<'resources, FRAME_CAPACITY>, RxEnqueueError> {
        let index = if let Some(index) = self.reserved_rx.take() {
            index
        } else {
            self.free_rx
                .try_receive()
                .map_err(|TryReceiveError::Empty| RxEnqueueError::QueueFull)?
        };
        Ok(self.rx_pool.claim_radio(index))
    }

    fn publish<R>(
        &self,
        lease: RxRadioLease<'resources, FRAME_CAPACITY>,
        length: usize,
        write: impl FnOnce(&mut [u8]) -> R,
    ) -> R {
        let (index, result) = lease.publish(length, write);
        self.publish_index(index);
        result
    }

    fn publish_index(&self, index: u8) {
        let published = if let Some(ordered) = self.ordered_rx {
            ordered.try_send(OrderedRxReady::owned(index)).is_ok()
        } else {
            self.ready_rx.try_send(index).is_ok()
        };
        if !published {
            unreachable!("one ordered or owned ready entry exists per non-free pinned RX slot");
        }
    }

    pub fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        Self::validate_length(frame.len()).map_err(RxEnqueueError::InvalidLength)?;
        let lease = self.try_claim_slot()?;
        self.publish(lease, frame.len(), |storage| storage.copy_from_slice(frame));
        Ok(())
    }

    /// Publish one contiguous Ethernet frame while exposing the exact edge
    /// before the claimed slot becomes visible to the network consumer.
    #[cfg(feature = "rx-delivery-observation")]
    pub fn try_send_observed(
        &mut self,
        frame: &[u8],
        before_publish: impl FnOnce(),
    ) -> Result<(), RxEnqueueError> {
        Self::validate_length(frame.len()).map_err(RxEnqueueError::InvalidLength)?;
        let lease = self.try_claim_slot()?;
        let (index, ()) = lease.publish(frame.len(), |storage| storage.copy_from_slice(frame));
        before_publish();
        self.publish_index(index);
        Ok(())
    }

    pub fn try_send_parts(
        &mut self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
    ) -> Result<(), RxEnqueueError> {
        let length = ETHERNET_HEADER_LEN
            .checked_add(payload.len())
            .ok_or(RxEnqueueError::InvalidLength(FrameLengthError::TooLong))?;
        Self::validate_length(length).map_err(RxEnqueueError::InvalidLength)?;
        let lease = self.try_claim_slot()?;
        self.publish(lease, length, |frame| {
            frame[..6].copy_from_slice(&destination);
            frame[6..12].copy_from_slice(&source);
            frame[12..14].copy_from_slice(&ether_type.to_be_bytes());
            frame[14..].copy_from_slice(payload);
        });
        Ok(())
    }

    /// Publish one Ethernet frame while exposing the exact ownership edge at
    /// which the claimed slot becomes visible to the network consumer.
    ///
    /// This method is absent from ordinary builds. `before_publish` runs after
    /// the frame copy but before insertion into `ready_rx`; failed admission
    /// never calls it.
    #[cfg(feature = "rx-delivery-observation")]
    pub fn try_send_parts_observed(
        &mut self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
        before_publish: impl FnOnce(),
    ) -> Result<(), RxEnqueueError> {
        let length = ETHERNET_HEADER_LEN
            .checked_add(payload.len())
            .ok_or(RxEnqueueError::InvalidLength(FrameLengthError::TooLong))?;
        Self::validate_length(length).map_err(RxEnqueueError::InvalidLength)?;
        let lease = self.try_claim_slot()?;
        let (index, ()) = lease.publish(length, |frame| {
            frame[..6].copy_from_slice(&destination);
            frame[6..12].copy_from_slice(&source);
            frame[12..14].copy_from_slice(&ether_type.to_be_bytes());
            frame[14..].copy_from_slice(payload);
        });
        before_publish();
        self.publish_index(index);
        Ok(())
    }

    pub async fn send(&mut self, frame: &[u8]) -> Result<(), FrameLengthError> {
        Self::validate_length(frame.len())?;
        self.wait_ready().await;
        self.try_send(frame)
            .expect("wait_ready reserved one pinned RX slot");
        Ok(())
    }

    /// Wait until at least one receive-queue owner is available.
    ///
    /// A protocol adapter can hold its independently staged radio frame while
    /// awaiting this edge, propagating bounded network backpressure instead of
    /// silently discarding a decoded Ethernet frame.
    pub async fn wait_ready(&mut self) {
        if self.reserved_rx.is_none() {
            self.reserved_rx = Some(self.free_rx.receive().await);
        }
    }

    pub fn free_capacity(&self) -> usize {
        self.free_rx.len() + usize::from(self.reserved_rx.is_some())
    }

    pub fn queue_len(&self) -> usize {
        self.ordered_rx
            .map_or_else(|| self.ready_rx.len(), |ready| ready.len())
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Drop
    for PinnedRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(index) = self.reserved_rx.take()
            && let Err(TrySendError::Full(_)) = self.free_rx_return.try_send(index)
        {
            unreachable!("reserved pinned RX index was lost");
        }
    }
}

pub struct SplitPinnedRadioRunner<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    free_rx: Receiver<'resources, M, u8, RX_QUEUE_DEPTH>,
    free_rx_return: Sender<'resources, M, u8, RX_QUEUE_DEPTH>,
    ready_rx: Sender<'resources, M, u8, RX_QUEUE_DEPTH>,
    ordered_rx: Option<Sender<'resources, M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>>,
    rx_pool: &'resources RxHandoffPool<FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    free_tx: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    ready_tx: Receiver<'resources, M, u8, TX_QUEUE_DEPTH>,
    ready_tx_return: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    tx_published: &'resources Signal<M, ()>,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    link: &'resources SharedLinkState<M>,
    hardware_address: &'resources SharedHardwareAddress<M>,
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
>
    SplitPinnedRadioRunner<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    /// Bind copied RX publications to the same typed frontier as shared
    /// staging slots. The matching device must be wrapped with
    /// [`SplitPinnedDevice::with_shared_rx`] using this exact consumer.
    pub fn with_shared_rx_ordering<const SHARED_CAPACITY: usize, const SHARED_SLOTS: usize>(
        mut self,
        shared: &SharedPinnedRxConsumer<'resources, M, SHARED_CAPACITY, SHARED_SLOTS>,
    ) -> Self {
        assert!(
            RX_QUEUE_DEPTH.saturating_add(SHARED_SLOTS) <= ORDERED_RX_READY_CAPACITY,
            "ordered RX frontier must cover every owned and shared slot"
        );
        assert!(
            RX_QUEUE_DEPTH <= usize::from(ORDERED_RX_SHARED_BIT)
                && SHARED_SLOTS <= usize::from(ORDERED_RX_SHARED_BIT),
            "ordered RX pool indices must fit in seven bits"
        );
        self.ordered_rx = Some(shared.ready_sender);
        self
    }

    /// Derive the receive-only capability before moving this runner into the
    /// production Wi-Fi event loop. The returned handle cannot observe or
    /// claim any network-owned TX slot.
    pub fn rx_publisher(&self) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        PinnedRxPublisher {
            free_rx: self.free_rx,
            free_rx_return: self.free_rx_return,
            ready_rx: self.ready_rx,
            ordered_rx: self.ordered_rx,
            rx_pool: self.rx_pool,
            reserved_rx: None,
        }
    }

    pub fn set_link_state(&self, state: LinkState) {
        self.link.set(state);
    }

    /// Select the MAC address for the next role while the persistent link is
    /// down. The next link-up wake makes `embassy-net` observe the new value.
    pub fn set_hardware_address(&self, address: [u8; 6]) {
        self.hardware_address.set(address);
    }

    pub fn try_send_rx(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        let mut publisher = self.rx_publisher();
        publisher.try_send(frame)
    }

    pub async fn send_rx(&self, frame: &[u8]) -> Result<(), FrameLengthError> {
        let mut publisher = self.rx_publisher();
        publisher.send(frame).await
    }

    /// Derive the TX-only capability used by a radio encoder or aggregate
    /// builder. Its type does not carry the unrelated receive queue depth.
    pub fn tx_consumer(
        &self,
    ) -> PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH> {
        PinnedTxConsumer {
            free_tx: self.free_tx,
            ready_tx: self.ready_tx,
            ready_tx_return: self.ready_tx_return,
            tx_pool: self.tx_pool,
        }
    }

    pub fn try_receive_tx(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        self.tx_consumer().try_receive()
    }

    pub async fn receive_tx(
        &self,
    ) -> PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH> {
        self.tx_consumer().receive().await
    }

    /// Wait for a network TX publication without claiming its pinned lease.
    /// This edge is cancellation-safe and lets a scheduler coalesce up to its
    /// negotiated aggregate target while RX/control remain selectable.
    pub async fn wait_tx_publication(&self) {
        self.tx_published.wait().await;
    }

    /// Wait until at least one ready TX lease exists without consuming it.
    pub async fn wait_tx_ready(&self) {
        self.ready_tx.ready_to_receive().await;
    }

    pub fn rx_queue_len(&self) -> usize {
        self.ready_rx.len()
    }

    pub fn tx_queue_len(&self) -> usize {
        self.ready_tx.len()
    }
}

/// Narrow radio-side capability for claiming ready network TX leases.
///
/// This value is cheap to copy from a [`SplitPinnedRadioRunner`] and is
/// independent of RX storage geometry. Aggregate construction may retain a
/// reference to it while claiming additional frames without gaining access to
/// link state or receive publication.
pub struct PinnedTxConsumer<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    free_tx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    ready_tx: Receiver<'resources, M, u8, QUEUE_DEPTH>,
    ready_tx_return: Sender<'resources, M, u8, QUEUE_DEPTH>,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub fn try_receive(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>> {
        match self.ready_tx.try_receive() {
            Ok(index) => Some(ReturningStableDmaBacking::new(
                self.tx_pool.claim_radio(index),
                PinnedTxReturn {
                    free_tx: self.free_tx,
                },
            )),
            Err(TryReceiveError::Empty) => None,
        }
    }

    pub async fn receive(
        &self,
    ) -> PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        let index = self.ready_tx.receive().await;
        ReturningStableDmaBacking::new(
            self.tx_pool.claim_radio(index),
            PinnedTxReturn {
                free_tx: self.free_tx,
            },
        )
    }

    pub fn queue_len(&self) -> usize {
        self.ready_tx.len()
    }

    /// Return a claimed but unmodified frame to the front-end ready queue.
    ///
    /// Aggregate builders use this when the next Ethernet frame belongs to a
    /// different peer. The claim removed one queue entry, so publishing the
    /// exact released index cannot exceed the bounded queue capacity.
    pub fn requeue(
        &self,
        frame: PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) {
        let index = frame.take_requeued_index();
        if let Err(TrySendError::Full(_)) = self.ready_tx_return.try_send(index) {
            unreachable!("a claimed pinned TX index leaves capacity for requeue");
        }
    }
}

/// Queue-return capability paired with a lower-level pinned DMA lease.
#[doc(hidden)]
pub struct PinnedTxReturn<'resources, M: RawMutex, const QUEUE_DEPTH: usize> {
    free_tx: Sender<'resources, M, u8, QUEUE_DEPTH>,
}

impl<M: RawMutex, const QUEUE_DEPTH: usize> DmaIndexReturn for PinnedTxReturn<'_, M, QUEUE_DEPTH> {
    fn return_index(&self, index: u8) {
        if let Err(TrySendError::Full(_)) = self.free_tx.try_send(index) {
            unreachable!("radio lease returns its unique pinned TX index");
        }
    }
}

/// Unique radio-side lease for one permanently located TX allocation.
///
/// Dropping the lease first releases DMA ownership and then returns the index
/// to `embassy-net`. Chip-specific MAC code retains this value through final
/// completion, BlockAck processing and any retry.
pub type PinnedTxFrame<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = ReturningStableDmaBacking<
    PinnedDmaTxRadioLease<'resources, FRAME_CAPACITY, HEADROOM, TRAILER>,
    PinnedTxReturn<'resources, M, QUEUE_DEPTH>,
>;
