//! Permanently located RX/TX slots for bounded, copy-minimal network ownership.

use core::{
    cell::UnsafeCell,
    marker::PhantomPinned,
    mem::MaybeUninit,
    pin::Pin,
    ptr,
    sync::atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
    task::{Context, Poll},
};

use embassy_net_driver::{Capabilities, Driver, HardwareAddress, LinkState};
use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender, TryReceiveError, TrySendError},
};

use crate::{ETHERNET_HEADER_LEN, FrameLengthError, RxEnqueueError, SharedLinkState};

const SLOT_FREE: u8 = 0;
const SLOT_NETWORK: u8 = 1;
const SLOT_READY: u8 = 2;
const SLOT_RADIO: u8 = 3;

#[repr(C, align(16))]
struct PinnedRxSlot<const FRAME_CAPACITY: usize> {
    bytes: UnsafeCell<[u8; FRAME_CAPACITY]>,
    length: AtomicUsize,
    state: AtomicU8,
}

impl<const FRAME_CAPACITY: usize> PinnedRxSlot<FRAME_CAPACITY> {
    const fn new() -> Self {
        Self {
            bytes: UnsafeCell::new([0; FRAME_CAPACITY]),
            length: AtomicUsize::new(0),
            state: AtomicU8::new(SLOT_FREE),
        }
    }

    fn claim_radio(&self) {
        assert_eq!(
            self.state
                .compare_exchange(SLOT_FREE, SLOT_RADIO, Ordering::AcqRel, Ordering::Acquire),
            Ok(SLOT_FREE),
            "free-channel entry did not name a free pinned RX slot"
        );
    }

    fn publish_ready(&self, length: usize) {
        self.length.store(length, Ordering::Relaxed);
        assert_eq!(
            self.state.compare_exchange(
                SLOT_RADIO,
                SLOT_READY,
                Ordering::Release,
                Ordering::Acquire
            ),
            Ok(SLOT_RADIO),
            "only the radio publisher may publish a pinned RX slot"
        );
    }

    fn claim_network(&self) {
        assert_eq!(
            self.state.compare_exchange(
                SLOT_READY,
                SLOT_NETWORK,
                Ordering::Acquire,
                Ordering::Acquire
            ),
            Ok(SLOT_READY),
            "ready-channel entry did not name a ready pinned RX slot"
        );
    }

    fn release_network(&self) {
        self.length.store(0, Ordering::Relaxed);
        assert_eq!(
            self.state.compare_exchange(
                SLOT_NETWORK,
                SLOT_FREE,
                Ordering::AcqRel,
                Ordering::Acquire
            ),
            Ok(SLOT_NETWORK),
            "only the network receive token may return this RX slot"
        );
    }

    fn length(&self) -> usize {
        self.length.load(Ordering::Acquire)
    }

    fn storage_mut_ptr(&self) -> *mut u8 {
        self.bytes.get().cast::<u8>()
    }
}

// SAFETY: the state machine and the free/ready index channels transfer unique
// ownership of `bytes` between one radio publisher and one network token.
// Acquire/release transitions publish the initialized length and contents.
unsafe impl<const FRAME_CAPACITY: usize> Sync for PinnedRxSlot<FRAME_CAPACITY> {}

#[repr(C)]
struct PinnedTxBytes<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> {
    headroom: [u8; HEADROOM],
    ethernet: [u8; FRAME_CAPACITY],
    trailer: [u8; TRAILER],
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    PinnedTxBytes<FRAME_CAPACITY, HEADROOM, TRAILER>
{
    const fn new() -> Self {
        Self {
            headroom: [0; HEADROOM],
            ethernet: [0; FRAME_CAPACITY],
            trailer: [0; TRAILER],
        }
    }
}

#[repr(C, align(16))]
struct PinnedTxSlot<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> {
    bytes: UnsafeCell<PinnedTxBytes<FRAME_CAPACITY, HEADROOM, TRAILER>>,
    length: AtomicUsize,
    state: AtomicU8,
}

impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize>
    PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>
{
    const fn new() -> Self {
        Self {
            bytes: UnsafeCell::new(PinnedTxBytes::new()),
            length: AtomicUsize::new(0),
            state: AtomicU8::new(SLOT_FREE),
        }
    }

    fn claim_network(&self) {
        assert_eq!(
            self.state.compare_exchange(
                SLOT_FREE,
                SLOT_NETWORK,
                Ordering::AcqRel,
                Ordering::Acquire
            ),
            Ok(SLOT_FREE),
            "free-channel entry did not name a free pinned TX slot"
        );
    }

    fn publish_ready(&self, length: usize) {
        self.length.store(length, Ordering::Relaxed);
        assert_eq!(
            self.state.compare_exchange(
                SLOT_NETWORK,
                SLOT_READY,
                Ordering::Release,
                Ordering::Acquire
            ),
            Ok(SLOT_NETWORK),
            "only the embassy-net token may publish a pinned TX slot"
        );
    }

    fn claim_radio(&self) {
        assert_eq!(
            self.state.compare_exchange(
                SLOT_READY,
                SLOT_RADIO,
                Ordering::Acquire,
                Ordering::Acquire
            ),
            Ok(SLOT_READY),
            "ready-channel entry did not name a ready pinned TX slot"
        );
    }

    fn release_network(&self) {
        assert_eq!(
            self.state.compare_exchange(
                SLOT_NETWORK,
                SLOT_FREE,
                Ordering::AcqRel,
                Ordering::Acquire
            ),
            Ok(SLOT_NETWORK),
            "only an unconsumed network token may return this TX slot"
        );
    }

    fn release_radio(&self) {
        assert_eq!(
            self.state
                .compare_exchange(SLOT_RADIO, SLOT_FREE, Ordering::AcqRel, Ordering::Acquire),
            Ok(SLOT_RADIO),
            "only the radio lease may return this TX slot"
        );
        self.length.store(0, Ordering::Relaxed);
    }

    fn length(&self) -> usize {
        self.length.load(Ordering::Acquire)
    }

    fn storage(&self) -> &[u8] {
        // SAFETY: callers hold either the unique network token or unique radio
        // lease selected by `state`. `PinnedTxBytes` is `repr(C)` and all
        // three adjacent fields have byte alignment, so the complete object
        // is one contiguous byte region without padding.
        unsafe {
            let bytes = &*self.bytes.get();
            debug_assert_eq!(
                core::mem::size_of_val(bytes),
                HEADROOM + FRAME_CAPACITY + TRAILER
            );
            core::slice::from_raw_parts(
                ptr::addr_of!(bytes.headroom).cast::<u8>(),
                HEADROOM + FRAME_CAPACITY + TRAILER,
            )
        }
    }

    fn storage_mut_ptr(&self) -> *mut u8 {
        self.bytes.get().cast::<u8>()
    }

    const fn storage_capacity(&self) -> usize {
        HEADROOM + FRAME_CAPACITY + TRAILER
    }
}

// SAFETY: `bytes` is accessed only by the single stage owner represented by
// `state`. Ownership moves through the bounded free/ready channels with
// acquire/release transitions. No public method can create two slot leases.
unsafe impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> Sync
    for PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>
{
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
    rx_slots: [PinnedRxSlot<FRAME_CAPACITY>; RX_QUEUE_DEPTH],
    free_tx: Channel<M, u8, TX_QUEUE_DEPTH>,
    ready_tx: Channel<M, u8, TX_QUEUE_DEPTH>,
    link: SharedLinkState<M>,
    split: AtomicBool,
}

/// Compatibility form with equal receive and transmit queue depths.
pub type PinnedResources<
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = SplitPinnedResources<M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH, QUEUE_DEPTH>;

/// Permanently located storage for the TX allocations exposed to radio DMA.
///
/// This is separate from [`PinnedResources`] so a platform linker can place
/// only the DMA-visible bytes in internal SRAM while keeping RX queues and
/// Embassy synchronization state in ordinary memory.
pub struct PinnedTxPool<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    slots: [PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>; QUEUE_DEPTH],
    _pin: PhantomPinned,
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            slots: [const { PinnedTxSlot::new() }; QUEUE_DEPTH],
            _pin: PhantomPinned,
        }
    }

    /// Initialize a large DMA pool directly in its final linker allocation.
    pub fn init_in_place(storage: &mut MaybeUninit<Self>) -> &mut Self {
        let storage = storage.as_mut_ptr();

        // SAFETY: `storage` is exclusively borrowed, aligned uninitialized
        // memory. Every slot and the pin marker are initialized before the
        // final reference is formed.
        unsafe {
            let slots =
                ptr::addr_of_mut!((*storage).slots)
                    .cast::<PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>>();
            for index in 0..QUEUE_DEPTH {
                slots.add(index).write(PinnedTxSlot::new());
            }
            ptr::addr_of_mut!((*storage)._pin).write(PhantomPinned);
            &mut *storage
        }
    }

    pub fn pin_static(storage: &'static mut Self) -> Pin<&'static mut Self> {
        // SAFETY: the unique static borrow is consumed by the returned pin.
        // `PhantomPinned` prevents safe extraction or movement thereafter.
        unsafe { Pin::new_unchecked(storage) }
    }
}

impl<
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> Default for PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

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
            rx_slots: [const { PinnedRxSlot::new() }; RX_QUEUE_DEPTH],
            free_tx: Channel::new(),
            ready_tx: Channel::new(),
            link: SharedLinkState::new(),
            split: AtomicBool::new(false),
        }
    }

    /// Initialize a potentially large pool directly in its final allocation.
    pub fn init_in_place(storage: &mut MaybeUninit<Self>) -> &mut Self {
        let storage = storage.as_mut_ptr();

        // SAFETY: `storage` is exclusively borrowed, aligned uninitialized
        // memory. Every field is initialized before the final reference is
        // formed. Slots are written one at a time, so no complete pool-sized
        // temporary is materialized on the embedded stack.
        unsafe {
            ptr::addr_of_mut!((*storage).free_rx).write(Channel::new());
            ptr::addr_of_mut!((*storage).ready_rx).write(Channel::new());
            let rx_slots =
                ptr::addr_of_mut!((*storage).rx_slots).cast::<PinnedRxSlot<FRAME_CAPACITY>>();
            for index in 0..RX_QUEUE_DEPTH {
                rx_slots.add(index).write(PinnedRxSlot::new());
            }
            ptr::addr_of_mut!((*storage).free_tx).write(Channel::new());
            ptr::addr_of_mut!((*storage).ready_tx).write(Channel::new());
            ptr::addr_of_mut!((*storage).link).write(SharedLinkState::new());
            ptr::addr_of_mut!((*storage).split).write(AtomicBool::new(false));
            &mut *storage
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
        // SAFETY: the pool stays pinned for `'resources`; only an immutable
        // slots reference escapes and no field is moved.
        let slots = &unsafe { pool.get_unchecked_mut() }.slots;
        let resources: &Self = self;

        (
            SplitPinnedDevice {
                ready_rx: resources.ready_rx.receiver(),
                free_rx: resources.free_rx.sender(),
                rx_slots: &resources.rx_slots,
                free_tx: resources.free_tx.receiver(),
                free_tx_return: resources.free_tx.sender(),
                ready_tx: resources.ready_tx.sender(),
                slots,
                link: &resources.link,
                station_address,
                reserved_tx: None,
                tx_reservation: (),
            },
            SplitPinnedRadioRunner {
                free_rx: resources.free_rx.receiver(),
                free_rx_return: resources.free_rx.sender(),
                ready_rx: resources.ready_rx.sender(),
                rx_slots: &resources.rx_slots,
                free_tx: resources.free_tx.sender(),
                ready_tx: resources.ready_tx.receiver(),
                slots,
                link: &resources.link,
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
    rx_slots: &'resources [PinnedRxSlot<FRAME_CAPACITY>; RX_QUEUE_DEPTH],
    free_tx: Receiver<'resources, M, u8, TX_QUEUE_DEPTH>,
    free_tx_return: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    ready_tx: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    slots: &'resources [PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>; TX_QUEUE_DEPTH],
    link: &'resources SharedLinkState<M>,
    station_address: [u8; 6],
    reserved_tx: Option<u8>,
    tx_reservation: (),
}

/// Compatibility form with equal receive and transmit queue depths.
pub type PinnedDevice<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = SplitPinnedDevice<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH, QUEUE_DEPTH>;

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
    fn poll_reserve_tx(&mut self, cx: &mut Context<'_>) -> bool {
        if self.reserved_tx.is_none()
            && let Poll::Ready(index) = self.free_tx.poll_receive(cx)
        {
            self.reserved_tx = Some(index);
        }
        self.reserved_tx.is_some()
    }

    fn take_tx_token<'device>(
        &'device mut self,
    ) -> PinnedTransmitToken<
        'device,
        'resources,
        M,
        FRAME_CAPACITY,
        HEADROOM,
        TRAILER,
        TX_QUEUE_DEPTH,
    > {
        let index = self
            .reserved_tx
            .take()
            .expect("TX token requires a reserved pool index");
        self.slots[usize::from(index)].claim_network();
        PinnedTransmitToken {
            free_tx: self.free_tx_return,
            ready_tx: self.ready_tx,
            slots: self.slots,
            index: Some(index),
            _reservation: &mut self.tx_reservation,
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
    slots: &'resources [PinnedRxSlot<FRAME_CAPACITY>; QUEUE_DEPTH],
    index: Option<u8>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> embassy_net_driver::RxToken
    for PinnedReceiveToken<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn consume<R, F>(self, f: F) -> R
    where
        F: FnOnce(&mut [u8]) -> R,
    {
        let index = self.index.expect("live pinned RX token");
        let slot = &self.slots[usize::from(index)];
        let length = slot.length();
        // SAFETY: this token is the unique SLOT_NETWORK owner selected by the
        // ready queue. The slot is returned only when `self` is dropped after
        // `f` completes.
        let frame = unsafe { core::slice::from_raw_parts_mut(slot.storage_mut_ptr(), length) };
        f(frame)
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Drop
    for PinnedReceiveToken<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(index) = self.index.take() {
            self.slots[usize::from(index)].release_network();
            if let Err(TrySendError::Full(_)) = self.free_rx.try_send(index) {
                unreachable!("network RX token returns its unique pinned index");
            }
        }
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
        if let Some(index) = self.reserved_tx.take()
            && let Err(TrySendError::Full(_)) = self.free_tx_return.try_send(index)
        {
            unreachable!("reserved pinned TX index was lost");
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
    slots: &'resources [PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>; QUEUE_DEPTH],
    index: Option<u8>,
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
        let index = self.index.take().expect("TX token consumed once");
        let slot = &self.slots[usize::from(index)];
        // SAFETY: consuming the unique network token gives this call exclusive
        // access to the slot selected by `index`. The slot remains pinned and
        // no radio lease exists before `publish_ready`.
        let storage = unsafe {
            core::slice::from_raw_parts_mut(slot.storage_mut_ptr(), slot.storage_capacity())
        };
        let result = f(&mut storage[HEADROOM..HEADROOM + length]);
        slot.publish_ready(length);
        if let Err(TrySendError::Full(_)) = self.ready_tx.try_send(index) {
            unreachable!("one ready entry exists per non-free pinned TX slot");
        }
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
        if let Some(index) = self.index.take() {
            self.slots[usize::from(index)].release_network();
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
        if !self.poll_reserve_tx(cx) {
            return None;
        }
        let index = match self.ready_rx.poll_receive(cx) {
            Poll::Ready(index) => index,
            Poll::Pending => return None,
        };
        self.rx_slots[usize::from(index)].claim_network();
        Some((
            PinnedReceiveToken {
                free_rx: self.free_rx,
                slots: self.rx_slots,
                index: Some(index),
            },
            self.take_tx_token(),
        ))
    }

    fn transmit(&mut self, cx: &mut Context<'_>) -> Option<Self::TxToken<'_>> {
        self.poll_reserve_tx(cx).then(|| self.take_tx_token())
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
        HardwareAddress::Ethernet(self.station_address)
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
    slots: &'resources [PinnedRxSlot<FRAME_CAPACITY>; QUEUE_DEPTH],
    reserved_rx: Option<u8>,
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    PinnedRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
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

    fn try_claim_slot(&mut self) -> Result<u8, RxEnqueueError> {
        let index = if let Some(index) = self.reserved_rx.take() {
            index
        } else {
            self.free_rx
                .try_receive()
                .map_err(|TryReceiveError::Empty| RxEnqueueError::QueueFull)?
        };
        self.slots[usize::from(index)].claim_radio();
        Ok(index)
    }

    fn publish(&self, index: u8, length: usize) {
        self.slots[usize::from(index)].publish_ready(length);
        if let Err(TrySendError::Full(_)) = self.ready_rx.try_send(index) {
            unreachable!("one ready entry exists per non-free pinned RX slot");
        }
    }

    pub fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        Self::validate_length(frame.len()).map_err(RxEnqueueError::InvalidLength)?;
        let index = self.try_claim_slot()?;
        let slot = &self.slots[usize::from(index)];
        // SAFETY: `try_claim_slot` changed this index to SLOT_RADIO, giving
        // this publisher exclusive access until `publish`.
        unsafe {
            core::slice::from_raw_parts_mut(slot.storage_mut_ptr(), frame.len())
                .copy_from_slice(frame);
        }
        self.publish(index, frame.len());
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
        let index = self.try_claim_slot()?;
        let slot = &self.slots[usize::from(index)];
        // SAFETY: `try_claim_slot` gave this publisher exclusive ownership.
        let frame = unsafe { core::slice::from_raw_parts_mut(slot.storage_mut_ptr(), length) };
        frame[..6].copy_from_slice(&destination);
        frame[6..12].copy_from_slice(&source);
        frame[12..14].copy_from_slice(&ether_type.to_be_bytes());
        frame[14..].copy_from_slice(payload);
        self.publish(index, length);
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
        self.ready_rx.len()
    }
}

impl<M: RawMutex, const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Drop
    for PinnedRxPublisher<'_, M, FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(index) = self.reserved_rx.take() {
            if let Err(TrySendError::Full(_)) = self.free_rx_return.try_send(index) {
                unreachable!("reserved pinned RX index was lost");
            }
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
    rx_slots: &'resources [PinnedRxSlot<FRAME_CAPACITY>; RX_QUEUE_DEPTH],
    free_tx: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    ready_tx: Receiver<'resources, M, u8, TX_QUEUE_DEPTH>,
    slots: &'resources [PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>; TX_QUEUE_DEPTH],
    link: &'resources SharedLinkState<M>,
}

/// Compatibility form with equal receive and transmit queue depths.
pub type PinnedRadioRunner<
    'resources,
    M,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> = SplitPinnedRadioRunner<
    'resources,
    M,
    FRAME_CAPACITY,
    HEADROOM,
    TRAILER,
    QUEUE_DEPTH,
    QUEUE_DEPTH,
>;

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
    /// Derive the receive-only capability before moving this runner into the
    /// production Wi-Fi event loop. The returned handle cannot observe or
    /// claim any network-owned TX slot.
    pub fn rx_publisher(&self) -> PinnedRxPublisher<'resources, M, FRAME_CAPACITY, RX_QUEUE_DEPTH> {
        PinnedRxPublisher {
            free_rx: self.free_rx,
            free_rx_return: self.free_rx_return,
            ready_rx: self.ready_rx,
            slots: self.rx_slots,
            reserved_rx: None,
        }
    }

    pub fn set_link_state(&self, state: LinkState) {
        self.link.set(state);
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
            slots: self.slots,
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
    slots: &'resources [PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>; QUEUE_DEPTH],
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
            Ok(index) => {
                self.slots[usize::from(index)].claim_radio();
                Some(PinnedTxFrame {
                    free_tx: self.free_tx,
                    slots: self.slots,
                    index: Some(index),
                })
            }
            Err(TryReceiveError::Empty) => None,
        }
    }

    pub async fn receive(
        &self,
    ) -> PinnedTxFrame<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH> {
        let index = self.ready_tx.receive().await;
        self.slots[usize::from(index)].claim_radio();
        PinnedTxFrame {
            free_tx: self.free_tx,
            slots: self.slots,
            index: Some(index),
        }
    }

    pub fn queue_len(&self) -> usize {
        self.ready_tx.len()
    }
}

/// Unique radio-side lease for one permanently located TX allocation.
///
/// Dropping the lease is the explicit ownership edge that returns the slot to
/// `embassy-net`. A chip-specific DMA wrapper must therefore retain this value
/// through completion, BlockAck processing and any retry.
pub struct PinnedTxFrame<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    free_tx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    slots: &'resources [PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>; QUEUE_DEPTH],
    index: Option<u8>,
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> PinnedTxFrame<'_, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn slot(&self) -> &PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER> {
        &self.slots[usize::from(self.index.expect("live pinned TX lease"))]
    }

    pub const fn ethernet_offset(&self) -> usize {
        HEADROOM
    }

    pub fn ethernet_length(&self) -> usize {
        self.slot().length()
    }

    /// Compatibility name for the complete Ethernet-frame length.
    pub fn len(&self) -> usize {
        self.ethernet_length()
    }

    pub fn is_empty(&self) -> bool {
        self.ethernet_length() == 0
    }

    pub fn ethernet(&self) -> &[u8] {
        let length = self.ethernet_length();
        &self.slot().storage()[HEADROOM..HEADROOM + length]
    }

    /// Compatibility name for the radio-owned Ethernet view.
    pub fn as_slice(&self) -> &[u8] {
        self.ethernet()
    }

    pub fn ethernet_mut(&mut self) -> &mut [u8] {
        let length = self.ethernet_length();
        &mut self.storage_mut()[HEADROOM..HEADROOM + length]
    }

    /// Complete headroom + Ethernet capacity + hardware trailer allocation.
    pub fn storage_mut(&mut self) -> &mut [u8] {
        let slot = self.slot();
        // SAFETY: `&mut self` is the unique live radio lease for this slot.
        // The state machine prevents a network token from existing until this
        // lease is dropped, and the backing pool remains pinned throughout.
        unsafe { core::slice::from_raw_parts_mut(slot.storage_mut_ptr(), slot.storage_capacity()) }
    }

    pub const fn trailer_capacity(&self) -> usize {
        TRAILER
    }
}

impl<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> Drop for PinnedTxFrame<'_, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(index) = self.index.take() {
            self.slots[usize::from(index)].release_radio();
            if let Err(TrySendError::Full(_)) = self.free_tx.try_send(index) {
                unreachable!("radio lease returns its unique pinned TX index");
            }
        }
    }
}
