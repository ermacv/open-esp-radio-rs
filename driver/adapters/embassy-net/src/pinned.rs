//! Permanently located RX/TX slots for bounded, copy-minimal network ownership.

use core::{
    pin::Pin,
    sync::atomic::{AtomicBool, AtomicU32, Ordering},
    task::{Context, Poll},
};

#[cfg(feature = "tx-phase-telemetry")]
use crate::tx_performance::{TX_PERFORMANCE, TxPerformanceSample};
use embassy_net_driver::{
    Capabilities, Checksum, ChecksumCapabilities, Driver, HardwareAddress, LinkState,
};
use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender, TryReceiveError, TrySendError},
    signal::Signal,
    waitqueue::GenericAtomicWaker,
};
use open_esp_radio_dma::{
    DmaIndexReturn, ExternalRxHandoffPool, ExternalRxNetworkLease, PinnedDmaTxNetworkLease,
    PinnedDmaTxPool, PinnedDmaTxRadioLease, ReturningStableDmaBacking, RxHandoffPool,
    RxNetworkLease, RxRadioLease, TaggedStableDmaBacking,
};

use crate::{ETHERNET_HEADER_LEN, FrameLengthError, RxEnqueueError, SharedLinkState};

/// Opaque identity of one logical network endpoint sharing a physical radio.
///
/// The network adapter preserves this value but never assigns Wi-Fi meaning
/// to it. The radio composition owns the mapping to STA, AP, or another VIF.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NetworkInterfaceId(u8);

impl NetworkInterfaceId {
    pub const fn new(value: u8) -> Self {
        Self(value)
    }

    pub const fn value(self) -> u8 {
        self.0
    }
}

const PINNED_TX_CREDIT_WAKER_SLOTS: usize = 8;

/// Per-endpoint notification for one shared physical TX credit pool.
///
/// `embassy_sync::Channel` intentionally stores one receiver waker. Multiple
/// logical network devices may consume this physical free queue, so polling
/// the channel directly would make their distinct task wakers replace and
/// wake each other forever while the queue is empty. The queue remains the
/// sole owner of free indices; this table wakes one active waiter on each real
/// credit-return edge.
struct PinnedTxCreditWakers<M: RawMutex> {
    slots: [GenericAtomicWaker<M>; PINNED_TX_CREDIT_WAKER_SLOTS],
}

impl<M: RawMutex> PinnedTxCreditWakers<M> {
    const fn new() -> Self {
        Self {
            slots: [const { GenericAtomicWaker::new(M::INIT) }; PINNED_TX_CREDIT_WAKER_SLOTS],
        }
    }

    fn register(&self, interface: NetworkInterfaceId, cx: &mut Context<'_>) {
        self.slots[usize::from(interface.value())].register(cx.waker());
    }

    fn wake_all(&self) {
        for slot in &self.slots {
            slot.wake();
        }
    }

    fn wake_waiter_after(
        &self,
        returned_by: NetworkInterfaceId,
        active: &AtomicU32,
        waiting: &AtomicU32,
    ) {
        let candidates = active.load(Ordering::Acquire) & waiting.load(Ordering::Acquire);
        let start = (usize::from(returned_by.value()) + 1) % PINNED_TX_CREDIT_WAKER_SLOTS;
        for offset in 0..PINNED_TX_CREDIT_WAKER_SLOTS {
            let index = (start + offset) % PINNED_TX_CREDIT_WAKER_SLOTS;
            if candidates & (1_u32 << index) != 0 {
                self.slots[index].wake();
                return;
            }
        }
    }

    fn validate(interface: NetworkInterfaceId) {
        assert!(
            usize::from(interface.value()) < PINNED_TX_CREDIT_WAKER_SLOTS,
            "network interface exceeds the physical TX credit notification table"
        );
    }
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
                pool: SharedPinnedRxPool::Copied(pool),
                on_release,
            },
        )
    }

    /// Split a queue whose indices retain original descriptor-backed buffers.
    pub fn split_external<'resources, const FRAME_CAPACITY: usize>(
        &'resources self,
        pool: &'resources ExternalRxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>,
        on_release: fn(),
    ) -> (
        SharedPinnedRxPublisher<'resources, M, SLOT_COUNT>,
        SharedPinnedRxConsumer<'resources, M, FRAME_CAPACITY, SLOT_COUNT>,
    ) {
        assert!(SLOT_COUNT > 0, "shared external RX pool must not be empty");
        assert!(
            SLOT_COUNT <= usize::from(ORDERED_RX_SHARED_BIT),
            "shared external RX index must fit in seven bits"
        );
        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "shared external RX queue may only be split once"
        );
        (
            SharedPinnedRxPublisher {
                ready: self.ready.sender(),
            },
            SharedPinnedRxConsumer {
                ready: self.ready.receiver(),
                ready_sender: self.ready.sender(),
                pool: SharedPinnedRxPool::External(pool),
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
    pool: SharedPinnedRxPool<'resources, FRAME_CAPACITY, SLOT_COUNT>,
    on_release: fn(),
}

enum SharedPinnedRxPool<'resources, const FRAME_CAPACITY: usize, const SLOT_COUNT: usize> {
    Copied(&'resources RxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>),
    External(&'resources ExternalRxHandoffPool<FRAME_CAPACITY, SLOT_COUNT>),
}

impl<'resources, const FRAME_CAPACITY: usize, const SLOT_COUNT: usize>
    SharedPinnedRxPool<'resources, FRAME_CAPACITY, SLOT_COUNT>
{
    fn claim_network(&self, index: u8) -> SharedPoolNetworkLease<'resources, FRAME_CAPACITY> {
        match self {
            Self::Copied(pool) => SharedPoolNetworkLease::Copied(pool.claim_network(index)),
            Self::External(pool) => SharedPoolNetworkLease::External(pool.claim_network(index)),
        }
    }
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
pub struct PinnedTxResources<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    free_tx: Channel<M, u8, TX_QUEUE_DEPTH>,
    /// Per-VIF FIFO publication frontiers sharing one finite physical credit
    /// pool. Cross-VIF order is chosen by the physical consumer; within each
    /// VIF, publication order is immutable.
    ready_tx: [Channel<M, u8, TX_QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    next_interface: AtomicU32,
    tx_published: Signal<M, ()>,
    tx_credit_wakers: PinnedTxCreditWakers<M>,
    tx_credit_waiters: AtomicU32,
    split: AtomicBool,
    /// Radio-owned link activity for each logical endpoint. A permanent
    /// network device may exist while its role is stopped, so credit sharing
    /// must follow the active owner graph rather than the static device count.
    tx_active: AtomicU32,
}

/// Static storage owned by one permanent logical network endpoint.
///
/// STA and AP must each have their own instance. Only RX ownership, link state
/// and the immutable Ethernet identity live here; physical TX storage belongs
/// to [`PinnedTxResources`] and is shared explicitly.
pub struct PinnedEndpointResources<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const RX_QUEUE_DEPTH: usize,
> {
    free_rx: Channel<M, u8, RX_QUEUE_DEPTH>,
    ready_rx: Channel<M, u8, RX_QUEUE_DEPTH>,
    rx_pool: RxHandoffPool<FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    link: SharedLinkState<M>,
    split: AtomicBool,
}

/// Permanently located storage for the TX allocations exposed to radio DMA.
///
/// This is separate from [`PinnedTxResources`] so a platform linker can place
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
    const TX_QUEUE_DEPTH: usize,
> PinnedTxResources<M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            free_tx: Channel::new(),
            ready_tx: [const { Channel::new() }; PINNED_TX_CREDIT_WAKER_SLOTS],
            next_interface: AtomicU32::new(0),
            tx_published: Signal::new(),
            tx_credit_wakers: PinnedTxCreditWakers::new(),
            tx_credit_waiters: AtomicU32::new(0),
            split: AtomicBool::new(false),
            tx_active: AtomicU32::new(0),
        }
    }

    pub fn split<'resources>(
        &'resources mut self,
        pool: Pin<&'resources mut PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>,
    ) -> (
        PinnedTxProvider<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) {
        assert!(TX_QUEUE_DEPTH > 0, "pinned TX pool must not be empty");
        assert!(
            TX_QUEUE_DEPTH <= usize::from(u8::MAX) + 1,
            "pinned TX pool index must fit in u8"
        );

        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "pinned resources may only be split once"
        );
        for index in 0..TX_QUEUE_DEPTH {
            self.free_tx
                .try_send(index as u8)
                .expect("an empty free queue accepts every pool index");
        }
        let pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH> =
            Pin::into_ref(pool).get_ref();
        let resources: &Self = self;

        (
            PinnedTxProvider {
                free_tx: resources.free_tx.receiver(),
                free_tx_return: resources.free_tx.sender(),
                ready_tx: &resources.ready_tx,
                tx_published: &resources.tx_published,
                tx_credit_wakers: &resources.tx_credit_wakers,
                tx_credit_waiters: &resources.tx_credit_waiters,
                tx_active: &resources.tx_active,
                tx_pool: pool,
            },
            PinnedTxConsumer {
                free_tx: resources.free_tx.sender(),
                ready_tx: &resources.ready_tx,
                next_interface: &resources.next_interface,
                tx_published: &resources.tx_published,
                tx_credit_wakers: &resources.tx_credit_wakers,
                tx_credit_waiters: &resources.tx_credit_waiters,
                tx_active: &resources.tx_active,
                tx_pool: pool,
            },
        )
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const TX_QUEUE_DEPTH: usize,
> Default for PinnedTxResources<M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const RX_QUEUE_DEPTH: usize>
    PinnedEndpointResources<M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            free_rx: Channel::new(),
            ready_rx: Channel::new(),
            rx_pool: RxHandoffPool::new(),
            link: SharedLinkState::new(),
            split: AtomicBool::new(false),
        }
    }

    pub fn split<
        'resources,
        const HEADROOM: usize,
        const TRAILER: usize,
        const TX_QUEUE_DEPTH: usize,
    >(
        &'resources mut self,
        tx: PinnedTxProvider<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
        interface: NetworkInterfaceId,
        hardware_address: [u8; 6],
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
        SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    ) {
        assert!(RX_QUEUE_DEPTH > 0, "pinned RX pool must not be empty");
        assert!(
            RX_QUEUE_DEPTH <= usize::from(u8::MAX) + 1,
            "pinned RX pool index must fit in u8"
        );
        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "pinned endpoint resources may only be split once"
        );
        PinnedTxCreditWakers::<M>::validate(interface);
        for index in 0..RX_QUEUE_DEPTH {
            self.free_rx
                .try_send(index as u8)
                .expect("an empty free RX queue accepts every pool index");
        }
        let resources: &Self = self;
        (
            SplitPinnedDevice {
                ready_rx: resources.ready_rx.receiver(),
                free_rx: resources.free_rx.sender(),
                rx_pool: &resources.rx_pool,
                free_tx: tx.free_tx,
                free_tx_return: tx.free_tx_return,
                ready_tx: tx.ready_tx,
                interface,
                tx_published: tx.tx_published,
                tx_credit_wakers: tx.tx_credit_wakers,
                tx_credit_waiters: tx.tx_credit_waiters,
                tx_active: tx.tx_active,
                tx_pool: tx.tx_pool,
                link: &resources.link,
                hardware_address,
                ingress_tx: None,
                application_tx: None,
                reserve_ingress_tx: false,
                waiting_for_tx_credit: false,
                checksum: ChecksumCapabilities::default(),
                tx_reservation: (),
            },
            SplitPinnedRxRunner {
                free_rx: resources.free_rx.receiver(),
                free_rx_return: resources.free_rx.sender(),
                ready_rx: resources.ready_rx.sender(),
                ordered_rx: None,
                rx_pool: &resources.rx_pool,
                link: &resources.link,
                tx_active: tx.tx_active,
                tx_interface: interface,
                tx_credit_wakers: tx.tx_credit_wakers,
            },
        )
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const RX_QUEUE_DEPTH: usize> Default
    for PinnedEndpointResources<M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Copyable authority for one logical endpoint to claim unique credits from
/// the shared physical TX pool and publish tagged ready entries.
pub struct PinnedTxProvider<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    free_tx: Receiver<'resources, M, u8, QUEUE_DEPTH>,
    free_tx_return: Sender<'resources, M, u8, QUEUE_DEPTH>,
    ready_tx: &'resources [Channel<M, u8, QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    tx_published: &'resources Signal<M, ()>,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_credit_waiters: &'resources AtomicU32,
    tx_active: &'resources AtomicU32,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Clone
    for PinnedTxProvider<'_, M, F, H, T, Q>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Copy
    for PinnedTxProvider<'_, M, F, H, T, Q>
{
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
    ready_tx: &'resources [Channel<M, u8, TX_QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    interface: NetworkInterfaceId,
    tx_published: &'resources Signal<M, ()>,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_credit_waiters: &'resources AtomicU32,
    tx_active: &'resources AtomicU32,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    link: &'resources SharedLinkState<M>,
    hardware_address: [u8; 6],
    /// One credit unavailable to ordinary egress and therefore available to
    /// satisfy the `Driver::receive` RX+TX-token contract under saturated TX.
    ingress_tx: Option<u8>,
    application_tx: Option<u8>,
    reserve_ingress_tx: bool,
    waiting_for_tx_credit: bool,
    checksum: ChecksumCapabilities,
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
    /// one credit per permanent endpoint beyond their advertised application
    /// capacity.
    pub fn with_ingress_tx_reserve(mut self) -> Self {
        assert!(
            TX_QUEUE_DEPTH > 1,
            "ingress TX reserve needs an application credit"
        );
        self.reserve_ingress_tx = true;
        self.ingress_tx = Some(
            self.try_take_free_tx()
                .expect("an ingress-enabled endpoint needs one dedicated TX credit"),
        );
        self
    }

    /// Override the checksum work advertised to the network stack.
    ///
    /// Selecting a mode which skips RX validation is sound only when a lower
    /// layer has already validated the corresponding packet checksum.
    pub fn with_checksum_capabilities(mut self, checksum: ChecksumCapabilities) -> Self {
        self.checksum = checksum;
        self
    }

    fn try_take_free_tx(&mut self) -> Option<u8> {
        let index = self.free_tx.try_receive().ok()?;
        if self.waiting_for_tx_credit {
            self.tx_credit_waiters
                .fetch_and(!(1_u32 << self.interface.value()), Ordering::AcqRel);
            self.waiting_for_tx_credit = false;
        }
        Some(index)
    }

    fn poll_free_tx(&mut self, cx: &mut Context<'_>) -> Poll<u8> {
        if let Some(index) = self.try_take_free_tx() {
            return Poll::Ready(index);
        }
        // Register outside Channel's single receiver-waker slot, then repeat
        // the ownership probe so a credit returned across the registration
        // edge cannot be lost.
        self.tx_credit_wakers.register(self.interface, cx);
        if !self.waiting_for_tx_credit {
            self.tx_credit_waiters
                .fetch_or(1_u32 << self.interface.value(), Ordering::AcqRel);
            self.waiting_for_tx_credit = true;
        }
        match self.try_take_free_tx() {
            Some(index) => Poll::Ready(index),
            None => Poll::Pending,
        }
    }

    fn poll_reserve_ingress_tx(&mut self, cx: &mut Context<'_>) -> bool {
        if self.ingress_tx.is_none()
            && let Poll::Ready(index) = self.poll_free_tx(cx)
        {
            self.ingress_tx = Some(index);
        }
        self.ingress_tx.is_some()
    }

    fn poll_reserve_application_tx(&mut self, cx: &mut Context<'_>) -> bool {
        if self.application_tx.is_some() {
            return true;
        }
        // Re-establish this endpoint's ingress credit before admitting more
        // application egress. Multiple permanent endpoints share the physical
        // pool, so protecting only the final global credit can starve one of
        // them indefinitely.
        if self.reserve_ingress_tx && !self.poll_reserve_ingress_tx(cx) {
            return false;
        }
        if let Poll::Ready(index) = self.poll_free_tx(cx) {
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
            interface: self.interface,
            tx_published: self.tx_published,
            tx_credit_wakers: self.tx_credit_wakers,
            tx_credit_waiters: self.tx_credit_waiters,
            tx_active: self.tx_active,
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
    lease: Option<SharedPoolNetworkLease<'resources, FRAME_CAPACITY>>,
    on_release: fn(),
}

enum SharedPoolNetworkLease<'resources, const FRAME_CAPACITY: usize> {
    Copied(RxNetworkLease<'resources, FRAME_CAPACITY>),
    External(ExternalRxNetworkLease<'resources, FRAME_CAPACITY>),
}

impl<const FRAME_CAPACITY: usize> SharedPoolNetworkLease<'_, FRAME_CAPACITY> {
    fn with_frame<R>(&mut self, f: impl FnOnce(&mut [u8]) -> R) -> R {
        match self {
            Self::Copied(lease) => lease.with_frame(f),
            Self::External(lease) => lease.with_frame(f),
        }
    }
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
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
    const SHARED_CAPACITY: usize,
    const SHARED_SLOTS: usize,
>
    SharedRxSplitPinnedDevice<
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
    /// Override checksum capabilities before constructing the IP stack.
    pub fn with_checksum_capabilities(mut self, checksum: ChecksumCapabilities) -> Self {
        self.inner = self.inner.with_checksum_capabilities(checksum);
        self
    }

    /// Select software IPv4/UDP validation for received packets.
    ///
    /// Disabling it is intended for a controlled diagnostic or for a future
    /// lower layer that can prove both checksums were already validated. TX
    /// checksum generation and all other protocol policies remain unchanged.
    pub fn with_software_ipv4_udp_rx_checksum_validation(self, enabled: bool) -> Self {
        let mut checksum = ChecksumCapabilities::default();
        if !enabled {
            checksum.ipv4 = Checksum::Tx;
            checksum.udp = Checksum::Tx;
        }
        self.with_checksum_capabilities(checksum)
    }

    /// Select software generation of the IPv4 UDP checksum.
    ///
    /// Disabling generation emits the RFC 768 zero-checksum representation
    /// and is intended only for a controlled cost diagnostic. The mandatory
    /// IPv4 header checksum and the selected RX checksum policy are preserved.
    pub fn with_software_ipv4_udp_tx_checksum_generation(mut self, enabled: bool) -> Self {
        let validate_rx = matches!(self.inner.checksum.udp, Checksum::Both | Checksum::Rx);
        self.inner.checksum.udp = match (validate_rx, enabled) {
            (true, true) => Checksum::Both,
            (true, false) => Checksum::Rx,
            (false, true) => Checksum::Tx,
            (false, false) => Checksum::None,
        };
        self
    }
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
        if self.waiting_for_tx_credit {
            self.tx_credit_waiters
                .fetch_and(!(1_u32 << self.interface.value()), Ordering::AcqRel);
            self.waiting_for_tx_credit = false;
        }
        for index in [self.ingress_tx.take(), self.application_tx.take()]
            .into_iter()
            .flatten()
        {
            if let Err(TrySendError::Full(_)) = self.free_tx_return.try_send(index) {
                unreachable!("reserved pinned TX index was lost");
            }
            self.tx_credit_wakers.wake_waiter_after(
                self.interface,
                self.tx_active,
                self.tx_credit_waiters,
            );
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
    ready_tx: &'resources [Channel<M, u8, QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    interface: NetworkInterfaceId,
    tx_published: &'resources Signal<M, ()>,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_credit_waiters: &'resources AtomicU32,
    tx_active: &'resources AtomicU32,
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
        #[cfg(feature = "tx-phase-telemetry")]
        let consume_started = TxPerformanceSample::read();
        assert!(
            length <= FRAME_CAPACITY,
            "embassy-net requested a frame larger than pinned driver capabilities"
        );
        let lease = self.lease.take().expect("TX token consumed once");
        #[cfg(feature = "tx-phase-telemetry")]
        let mut emitted = TxPerformanceSample::default();
        let (index, result) = lease.publish(length, |buffer| {
            #[cfg(feature = "tx-phase-telemetry")]
            let started = TxPerformanceSample::read();
            let result = f(buffer);
            #[cfg(feature = "tx-phase-telemetry")]
            {
                emitted = TxPerformanceSample::read().wrapping_delta_since(started);
            }
            result
        });
        let ready = &self.ready_tx[usize::from(self.interface.value())];
        if let Err(TrySendError::Full(_)) = ready.try_send(index) {
            unreachable!("one ready entry exists per non-free pinned TX slot");
        }
        self.tx_published.signal(());
        #[cfg(feature = "tx-phase-telemetry")]
        TX_PERFORMANCE.record_consume(
            length,
            consume_started,
            emitted,
            TxPerformanceSample::read(),
        );
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
            self.tx_credit_wakers.wake_waiter_after(
                self.interface,
                self.tx_active,
                self.tx_credit_waiters,
            );
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
        #[cfg(feature = "tx-phase-telemetry")]
        let started = TxPerformanceSample::read();
        let token = if !self.poll_reserve_application_tx(cx) {
            None
        } else {
            let index = self
                .application_tx
                .take()
                .expect("application admission reserves one TX credit");
            Some(self.take_tx_token(index))
        };
        #[cfg(feature = "tx-phase-telemetry")]
        TX_PERFORMANCE.record_admission(started, TxPerformanceSample::read(), token.is_some());
        token
    }

    fn link_state(&mut self, cx: &mut Context<'_>) -> LinkState {
        self.link.get(cx)
    }

    fn capabilities(&self) -> Capabilities {
        let mut capabilities = Capabilities::default();
        capabilities.max_transmission_unit = FRAME_CAPACITY;
        capabilities.max_burst_size = Some(RX_QUEUE_DEPTH.min(TX_QUEUE_DEPTH));
        capabilities.checksum = self.checksum.clone();
        capabilities
    }

    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ethernet(self.hardware_address)
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
/// moved into an RX protocol sink while [`SplitPinnedRxRunner`] remains the
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
    #[cfg(feature = "diagnostics")]
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
    #[cfg(feature = "diagnostics")]
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

pub struct SplitPinnedRxRunner<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const RX_QUEUE_DEPTH: usize,
> {
    free_rx: Receiver<'resources, M, u8, RX_QUEUE_DEPTH>,
    free_rx_return: Sender<'resources, M, u8, RX_QUEUE_DEPTH>,
    ready_rx: Sender<'resources, M, u8, RX_QUEUE_DEPTH>,
    ordered_rx: Option<Sender<'resources, M, OrderedRxReady, ORDERED_RX_READY_CAPACITY>>,
    rx_pool: &'resources RxHandoffPool<FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    link: &'resources SharedLinkState<M>,
    tx_active: &'resources AtomicU32,
    tx_interface: NetworkInterfaceId,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
}

impl<'resources, M: RawMutex, const FRAME_CAPACITY: usize, const RX_QUEUE_DEPTH: usize>
    SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>
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
        if state == LinkState::Up {
            // Make this endpoint eligible for returned-credit notification
            // before it becomes visible to its network stack.
            self.tx_active
                .fetch_or(1_u32 << self.tx_interface.value(), Ordering::AcqRel);
            self.tx_credit_wakers.wake_all();
            self.link.set(state);
        } else {
            // Stop network admission before removing the endpoint from the
            // returned-credit notification set.
            self.link.set(state);
            self.tx_active
                .fetch_and(!(1_u32 << self.tx_interface.value()), Ordering::AcqRel);
            self.tx_credit_wakers.wake_all();
        }
    }

    pub fn try_send_rx(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        let mut publisher = self.rx_publisher();
        publisher.try_send(frame)
    }

    pub async fn send_rx(&self, frame: &[u8]) -> Result<(), FrameLengthError> {
        let mut publisher = self.rx_publisher();
        publisher.send(frame).await
    }

    pub fn rx_queue_len(&self) -> usize {
        self.ready_rx.len()
    }
}

/// Narrow radio-side capability for claiming ready network TX leases.
///
/// This value is the sole radio-side consumer created by [`PinnedTxResources`]
/// and is
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
    ready_tx: &'resources [Channel<M, u8, QUEUE_DEPTH>; PINNED_TX_CREDIT_WAKER_SLOTS],
    next_interface: &'resources AtomicU32,
    tx_published: &'resources Signal<M, ()>,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_credit_waiters: &'resources AtomicU32,
    tx_active: &'resources AtomicU32,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
}

/// TX consumer narrowed to one logical interface.
///
/// Aggregate encoders receive this capability instead of the physical
/// consumer. They may extend a batch from their immutable per-VIF FIFO, but
/// can never claim a lease published by another VIF sharing the hardware.
pub struct PinnedTxInterfaceConsumer<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    physical: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    interface: NetworkInterfaceId,
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Clone
    for PinnedTxInterfaceConsumer<'_, M, F, H, T, Q>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Copy
    for PinnedTxInterfaceConsumer<'_, M, F, H, T, Q>
{
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Clone
    for PinnedTxConsumer<'_, M, F, H, T, Q>
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const F: usize, const H: usize, const T: usize, const Q: usize> Copy
    for PinnedTxConsumer<'_, M, F, H, T, Q>
{
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
    fn claim(
        &self,
        interface: NetworkInterfaceId,
        index: u8,
    ) -> PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        TaggedStableDmaBacking::new(
            interface,
            ReturningStableDmaBacking::new(
                self.tx_pool.claim_radio(index),
                PinnedTxReturn {
                    free_tx: self.free_tx,
                    interface,
                    tx_credit_wakers: self.tx_credit_wakers,
                    tx_credit_waiters: self.tx_credit_waiters,
                    tx_active: self.tx_active,
                },
            ),
        )
    }

    pub const fn for_interface(
        self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    {
        PinnedTxInterfaceConsumer {
            physical: self,
            interface,
        }
    }

    pub fn try_receive(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>> {
        let start = self.next_interface.fetch_add(1, Ordering::Relaxed) as usize
            % PINNED_TX_CREDIT_WAKER_SLOTS;
        for offset in 0..PINNED_TX_CREDIT_WAKER_SLOTS {
            let interface = (start + offset) % PINNED_TX_CREDIT_WAKER_SLOTS;
            if let Ok(index) = self.ready_tx[interface].try_receive() {
                self.next_interface.store(
                    ((interface + 1) % PINNED_TX_CREDIT_WAKER_SLOTS) as u32,
                    Ordering::Relaxed,
                );
                return Some(self.claim(NetworkInterfaceId::new(interface as u8), index));
            }
        }
        None
    }

    pub async fn receive(
        &self,
    ) -> PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        loop {
            if let Some(frame) = self.try_receive() {
                return frame;
            }
            self.wait_publication().await;
        }
    }

    pub fn queue_len(&self) -> usize {
        self.ready_tx.iter().map(Channel::len).sum()
    }

    /// Claim the oldest frame published by one logical interface.
    pub fn try_receive_for(
        &self,
        interface: NetworkInterfaceId,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>> {
        let index = self.ready_tx[usize::from(interface.value())]
            .try_receive()
            .ok()?;
        Some(self.claim(interface, index))
    }

    /// Count the immutable FIFO frontier for one logical interface.
    pub fn queue_len_for(&self, interface: NetworkInterfaceId) -> usize {
        self.ready_tx[usize::from(interface.value())].len()
    }

    pub async fn receive_for(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        loop {
            if let Some(frame) = self.try_receive_for(interface) {
                return frame;
            }
            self.wait_publication().await;
        }
    }

    pub async fn wait_ready_for(&self, interface: NetworkInterfaceId) {
        loop {
            if self.queue_len_for(interface) != 0 {
                return;
            }
            self.wait_publication().await;
        }
    }

    pub async fn wait_publication(&self) {
        self.tx_published.wait().await;
    }

    pub async fn wait_ready(&self) {
        loop {
            if self.queue_len() != 0 {
                return;
            }
            self.wait_publication().await;
        }
    }
}

impl<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub const fn interface(self) -> NetworkInterfaceId {
        self.interface
    }

    pub fn try_receive(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>> {
        let frame = self.physical.try_receive_for(self.interface);
        frame.inspect(|frame| {
            assert_eq!(
                *frame.tag(),
                self.interface,
                "interface-narrowed TX endpoint received another VIF's lease"
            );
        })
    }

    pub async fn receive(
        &self,
    ) -> PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        loop {
            if let Some(frame) = self.try_receive() {
                return frame;
            }
            self.physical.wait_publication().await;
        }
    }

    pub fn queue_len(&self) -> usize {
        self.physical.queue_len_for(self.interface)
    }

    pub async fn wait_ready(&self) {
        loop {
            if self.queue_len() != 0 {
                return;
            }
            self.physical.wait_publication().await;
        }
    }
}

/// Explicit single-endpoint radio composition.
///
/// Resource ownership remains split: the RX runner belongs to one permanent
/// endpoint while the TX consumer belongs to the physical fabric. This
/// convenience owner is useful for single-VIF schedulers and can be replaced
/// by a multi-endpoint scheduler without recreating either resource.
pub struct PinnedNetworkRunner<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    interface: NetworkInterfaceId,
    rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    tx: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
}

/// One physical radio-side owner for two permanent logical network endpoints.
///
/// RX and link-state publication are addressed by logical interface. TX is
/// never duplicated or filtered: both network devices publish tagged leases
/// into the single consumer retained by this value, and the Wi-Fi scheduler
/// must dispatch every tag to its matching role encoder.
pub struct DualPinnedNetworkRunner<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const RX_QUEUE_DEPTH: usize,
    const TX_QUEUE_DEPTH: usize,
> {
    first_interface: NetworkInterfaceId,
    first_rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    second_interface: NetworkInterfaceId,
    second_rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    tx: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
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
    DualPinnedNetworkRunner<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    pub fn new(
        first_interface: NetworkInterfaceId,
        first_rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
        second_interface: NetworkInterfaceId,
        second_rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
        tx: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) -> Self {
        assert_ne!(
            first_interface, second_interface,
            "dual network endpoints require distinct interface identities"
        );
        Self {
            first_interface,
            first_rx,
            second_interface,
            second_rx,
            tx,
        }
    }

    pub const fn first_interface(&self) -> NetworkInterfaceId {
        self.first_interface
    }

    pub const fn second_interface(&self) -> NetworkInterfaceId {
        self.second_interface
    }

    fn rx_for(
        &self,
        interface: NetworkInterfaceId,
    ) -> &SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        if interface == self.first_interface {
            &self.first_rx
        } else if interface == self.second_interface {
            &self.second_rx
        } else {
            panic!("network interface does not belong to this radio owner")
        }
    }

    pub fn with_shared_rx_ordering<
        const FIRST_SHARED_CAPACITY: usize,
        const FIRST_SHARED_SLOTS: usize,
        const SECOND_SHARED_CAPACITY: usize,
        const SECOND_SHARED_SLOTS: usize,
    >(
        self,
        first: &SharedPinnedRxConsumer<'resources, M, FIRST_SHARED_CAPACITY, FIRST_SHARED_SLOTS>,
        second: &SharedPinnedRxConsumer<'resources, M, SECOND_SHARED_CAPACITY, SECOND_SHARED_SLOTS>,
    ) -> Self {
        Self {
            first_rx: self.first_rx.with_shared_rx_ordering(first),
            second_rx: self.second_rx.with_shared_rx_ordering(second),
            ..self
        }
    }

    pub fn rx_publisher(
        &self,
        interface: NetworkInterfaceId,
    ) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        self.rx_for(interface).rx_publisher()
    }

    pub fn set_link_state(&self, interface: NetworkInterfaceId, state: LinkState) {
        self.rx_for(interface).set_link_state(state);
    }

    pub fn try_receive_tx(
        &self,
    ) -> Option<PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>>
    {
        self.tx.try_receive()
    }

    pub async fn receive_tx(
        &self,
    ) -> PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH> {
        self.tx.receive().await
    }

    pub const fn tx_consumer(
        &self,
    ) -> PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH> {
        self.tx
    }

    pub async fn wait_tx_publication(&self) {
        self.tx.wait_publication().await;
    }

    pub async fn wait_tx_ready(&self) {
        self.tx.wait_ready().await;
    }

    pub fn tx_queue_len(&self) -> usize {
        self.tx.queue_len()
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
>
    PinnedNetworkRunner<
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        RX_QUEUE_DEPTH,
        TX_QUEUE_DEPTH,
    >
{
    pub const fn new(
        interface: NetworkInterfaceId,
        rx: SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
        tx: PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) -> Self {
        Self { interface, rx, tx }
    }

    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    pub fn into_parts(
        self,
    ) -> (
        SplitPinnedRxRunner<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH>,
        PinnedTxConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    ) {
        (self.rx, self.tx)
    }

    pub fn with_shared_rx_ordering<const SHARED_CAPACITY: usize, const SHARED_SLOTS: usize>(
        self,
        shared: &SharedPinnedRxConsumer<'resources, M, SHARED_CAPACITY, SHARED_SLOTS>,
    ) -> Self {
        Self {
            interface: self.interface,
            rx: self.rx.with_shared_rx_ordering(shared),
            tx: self.tx,
        }
    }

    pub fn rx_publisher(&self) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        self.rx.rx_publisher()
    }

    pub fn set_link_state(&self, state: LinkState) {
        self.rx.set_link_state(state);
    }

    pub fn try_send_rx(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.rx.try_send_rx(frame)
    }

    pub async fn send_rx(&self, frame: &[u8]) -> Result<(), FrameLengthError> {
        self.rx.send_rx(frame).await
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

    pub const fn tx_consumer(
        &self,
    ) -> PinnedTxInterfaceConsumer<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>
    {
        self.tx.for_interface(self.interface)
    }

    pub async fn wait_tx_publication(&self) {
        self.tx.wait_publication().await;
    }

    pub async fn wait_tx_ready(&self) {
        self.tx_consumer().wait_ready().await;
    }

    pub fn rx_queue_len(&self) -> usize {
        self.rx.rx_queue_len()
    }

    pub fn tx_queue_len(&self) -> usize {
        self.tx_consumer().queue_len()
    }
}

/// Queue-return capability paired with a lower-level pinned DMA lease.
#[doc(hidden)]
pub struct PinnedTxReturn<'resources, M: RawMutex, const QUEUE_DEPTH: usize> {
    free_tx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    interface: NetworkInterfaceId,
    tx_credit_wakers: &'resources PinnedTxCreditWakers<M>,
    tx_credit_waiters: &'resources AtomicU32,
    tx_active: &'resources AtomicU32,
}

impl<M: RawMutex, const QUEUE_DEPTH: usize> DmaIndexReturn for PinnedTxReturn<'_, M, QUEUE_DEPTH> {
    fn return_index(&self, index: u8) {
        if let Err(TrySendError::Full(_)) = self.free_tx.try_send(index) {
            unreachable!("radio lease returns its unique pinned TX index");
        }
        // A terminal A-MPDU releases its retained leases synchronously. The
        // first returned index changes the physical pool from empty to ready;
        // the remaining indices are additional credits, not additional
        // readiness edges. If another core drains the pool concurrently, a
        // later return legitimately creates a new edge and wakes again.
        if self.free_tx.len() == 1 {
            self.tx_credit_wakers.wake_waiter_after(
                self.interface,
                self.tx_active,
                self.tx_credit_waiters,
            );
        }
    }
}

/// Unique radio-side lease for one permanently located TX allocation.
///
/// Dropping the lease first releases DMA ownership and then returns the index
/// to `embassy-net`. Chip-specific MAC code retains this value through final
/// completion, BlockAck processing and any retry.
type PinnedTxBacking<
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

/// One network-published DMA frame plus the logical endpoint that published
/// it. The tag remains outside the DMA allocation and must be consumed before
/// role-specific encoding begins.
pub type PinnedTxFrame<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = TaggedStableDmaBacking<
    NetworkInterfaceId,
    PinnedTxBacking<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
>;
