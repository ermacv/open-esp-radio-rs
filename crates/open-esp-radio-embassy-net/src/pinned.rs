//! Permanently located TX slots for referenced/cache-TX operation.

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

use crate::{EthernetFrame, FrameLengthError, RxEnqueueError, SharedLinkState};

const SLOT_FREE: u8 = 0;
const SLOT_NETWORK: u8 = 1;
const SLOT_READY: u8 = 2;
const SLOT_RADIO: u8 = 3;

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

    fn storage_mut(&self) -> &mut [u8] {
        // SAFETY: the slot-state/channel protocol gives the caller exclusive
        // ownership. Stable storage belongs to a pinned `PinnedResources` and
        // cannot be moved through any public API.
        unsafe {
            let bytes = &mut *self.bytes.get();
            debug_assert_eq!(
                core::mem::size_of_val(bytes),
                HEADROOM + FRAME_CAPACITY + TRAILER
            );
            core::slice::from_raw_parts_mut(
                ptr::addr_of_mut!(bytes.headroom).cast::<u8>(),
                HEADROOM + FRAME_CAPACITY + TRAILER,
            )
        }
    }
}

// SAFETY: `bytes` is accessed only by the single stage owner represented by
// `state`. Ownership moves through the bounded free/ready channels with
// acquire/release transitions. No public method can create two slot leases.
unsafe impl<const FRAME_CAPACITY: usize, const HEADROOM: usize, const TRAILER: usize> Sync
    for PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>
{
}

/// Static resources for a copy-free `embassy-net` TX ownership boundary.
///
/// [`PinnedTxPool`] owns the contiguous slots. `embassy-net` sees only each
/// slot's middle Ethernet region; the radio lease sees the complete allocation
/// and remains its unique owner until dropped. The pool must be pinned before
/// [`Self::split`].
///
/// SOURCE: complete `_oracles/libnet80211.a[ieee80211_output.o]::
/// ieee80211_alloc_tx_buf` cache-TX/type-nine path and complete
/// `_oracles/libpp.a[esf_buf.o]::{esf_buf_setup,esf_buf_alloc}`.
pub struct PinnedResources<
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    rx: Channel<M, EthernetFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    free_tx: Channel<M, u8, QUEUE_DEPTH>,
    ready_tx: Channel<M, u8, QUEUE_DEPTH>,
    link: SharedLinkState<M>,
    split: AtomicBool,
}

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
        const QUEUE_DEPTH: usize,
    > PinnedResources<M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            rx: Channel::new(),
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
            ptr::addr_of_mut!((*storage).rx).write(Channel::new());
            ptr::addr_of_mut!((*storage).free_tx).write(Channel::new());
            ptr::addr_of_mut!((*storage).ready_tx).write(Channel::new());
            ptr::addr_of_mut!((*storage).link).write(SharedLinkState::new());
            ptr::addr_of_mut!((*storage).split).write(AtomicBool::new(false));
            &mut *storage
        }
    }

    pub fn split<'resources>(
        &'resources mut self,
        pool: Pin<&'resources mut PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>>,
        station_address: [u8; 6],
    ) -> (
        PinnedDevice<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
        PinnedRadioRunner<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>,
    ) {
        assert!(QUEUE_DEPTH > 0, "pinned TX pool must not be empty");
        assert!(
            QUEUE_DEPTH <= usize::from(u8::MAX) + 1,
            "pinned TX pool index must fit in u8"
        );

        assert!(
            !self.split.swap(true, Ordering::AcqRel),
            "pinned resources may only be split once"
        );
        for index in 0..QUEUE_DEPTH {
            self.free_tx
                .try_send(index as u8)
                .expect("an empty free queue accepts every pool index");
        }
        // SAFETY: the pool stays pinned for `'resources`; only an immutable
        // slots reference escapes and no field is moved.
        let slots = &unsafe { pool.get_unchecked_mut() }.slots;
        let resources: &Self = self;

        (
            PinnedDevice {
                rx: resources.rx.receiver(),
                free_tx: resources.free_tx.receiver(),
                free_tx_return: resources.free_tx.sender(),
                ready_tx: resources.ready_tx.sender(),
                slots,
                link: &resources.link,
                station_address,
                reserved_tx: None,
                tx_reservation: (),
            },
            PinnedRadioRunner {
                rx: resources.rx.sender(),
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
        const QUEUE_DEPTH: usize,
    > Default for PinnedResources<M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

pub struct PinnedDevice<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    rx: Receiver<'resources, M, EthernetFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    free_tx: Receiver<'resources, M, u8, QUEUE_DEPTH>,
    free_tx_return: Sender<'resources, M, u8, QUEUE_DEPTH>,
    ready_tx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    slots: &'resources [PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>; QUEUE_DEPTH],
    link: &'resources SharedLinkState<M>,
    station_address: [u8; 6],
    reserved_tx: Option<u8>,
    tx_reservation: (),
}

impl<
        'resources,
        M: RawMutex,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const QUEUE_DEPTH: usize,
    > PinnedDevice<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn poll_reserve_tx(&mut self, cx: &mut Context<'_>) -> bool {
        if self.reserved_tx.is_none() {
            if let Poll::Ready(index) = self.free_tx.poll_receive(cx) {
                self.reserved_tx = Some(index);
            }
        }
        self.reserved_tx.is_some()
    }

    fn take_tx_token<'device>(
        &'device mut self,
    ) -> PinnedTransmitToken<'device, 'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    {
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

impl<
        M: RawMutex,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const QUEUE_DEPTH: usize,
    > Drop for PinnedDevice<'_, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    fn drop(&mut self) {
        if let Some(index) = self.reserved_tx.take() {
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
        let storage = slot.storage_mut();
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
        const QUEUE_DEPTH: usize,
    > Driver for PinnedDevice<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    type RxToken<'device>
        = crate::ReceiveToken<FRAME_CAPACITY>
    where
        Self: 'device;
    type TxToken<'device>
        =
        PinnedTransmitToken<'device, 'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
    where
        Self: 'device;

    fn receive(&mut self, cx: &mut Context<'_>) -> Option<(Self::RxToken<'_>, Self::TxToken<'_>)> {
        if !self.poll_reserve_tx(cx) {
            return None;
        }
        let frame = match self.rx.poll_receive(cx) {
            Poll::Ready(frame) => frame,
            Poll::Pending => return None,
        };
        Some((crate::ReceiveToken { frame }, self.take_tx_token()))
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
        capabilities.max_burst_size = Some(QUEUE_DEPTH);
        capabilities
    }

    fn hardware_address(&self) -> HardwareAddress {
        HardwareAddress::Ethernet(self.station_address)
    }
}

pub struct PinnedRadioRunner<
    'resources,
    M: RawMutex,
    const FRAME_CAPACITY: usize,
    const HEADROOM: usize,
    const TRAILER: usize,
    const QUEUE_DEPTH: usize,
> {
    rx: Sender<'resources, M, EthernetFrame<FRAME_CAPACITY>, QUEUE_DEPTH>,
    free_tx: Sender<'resources, M, u8, QUEUE_DEPTH>,
    ready_tx: Receiver<'resources, M, u8, QUEUE_DEPTH>,
    slots: &'resources [PinnedTxSlot<FRAME_CAPACITY, HEADROOM, TRAILER>; QUEUE_DEPTH],
    link: &'resources SharedLinkState<M>,
}

impl<
        'resources,
        M: RawMutex,
        const FRAME_CAPACITY: usize,
        const HEADROOM: usize,
        const TRAILER: usize,
        const QUEUE_DEPTH: usize,
    > PinnedRadioRunner<'resources, M, FRAME_CAPACITY, HEADROOM, TRAILER, QUEUE_DEPTH>
{
    pub fn set_link_state(&self, state: LinkState) {
        self.link.set(state);
    }

    pub fn try_send_rx(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        let owned = EthernetFrame::copy_from_slice(frame).map_err(RxEnqueueError::InvalidLength)?;
        self.rx.try_send(owned).map_err(|error| match error {
            TrySendError::Full(_) => RxEnqueueError::QueueFull,
        })
    }

    pub async fn send_rx(&self, frame: &[u8]) -> Result<(), FrameLengthError> {
        let owned = EthernetFrame::copy_from_slice(frame)?;
        self.rx.send(owned).await;
        Ok(())
    }

    pub fn try_receive_tx(
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

    pub async fn receive_tx(
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

    pub fn rx_queue_len(&self) -> usize {
        self.rx.len()
    }

    pub fn tx_queue_len(&self) -> usize {
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
        &mut self.slot().storage_mut()[HEADROOM..HEADROOM + length]
    }

    /// Complete headroom + Ethernet capacity + hardware trailer allocation.
    pub fn storage_mut(&mut self) -> &mut [u8] {
        self.slot().storage_mut()
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
