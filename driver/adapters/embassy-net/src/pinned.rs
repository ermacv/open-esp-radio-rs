//! Permanently located RX/TX slots for bounded, copy-minimal network ownership.

use core::{
    pin::Pin,
    sync::atomic::{AtomicBool, Ordering},
    task::{Context, Poll},
};

use embassy_net_driver::{Capabilities, Driver, HardwareAddress, LinkState};
use embassy_sync::{
    blocking_mutex::raw::RawMutex,
    channel::{Channel, Receiver, Sender, TryReceiveError, TrySendError},
};
use open_esp_radio_dma::{
    DmaIndexReturn, PinnedDmaTxNetworkLease, PinnedDmaTxPool, PinnedDmaTxRadioLease,
    ReturningStableDmaBacking, RxHandoffPool, RxNetworkLease, RxRadioLease,
};

use crate::{
    ETHERNET_HEADER_LEN, FrameLengthError, IngressPollFairness, RxEnqueueError, SharedLinkState,
};

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
            link: SharedLinkState::new(),
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

        (
            SplitPinnedDevice {
                ready_rx: resources.ready_rx.receiver(),
                free_rx: resources.free_rx.sender(),
                rx_pool: &resources.rx_pool,
                free_tx: resources.free_tx.receiver(),
                free_tx_return: resources.free_tx.sender(),
                ready_tx: resources.ready_tx.sender(),
                tx_pool: pool,
                link: &resources.link,
                station_address,
                reserved_tx: None,
                tx_reservation: (),
                ingress_fairness: IngressPollFairness::new(RX_QUEUE_DEPTH.min(TX_QUEUE_DEPTH)),
            },
            SplitPinnedRadioRunner {
                free_rx: resources.free_rx.receiver(),
                free_rx_return: resources.free_rx.sender(),
                ready_rx: resources.ready_rx.sender(),
                rx_pool: &resources.rx_pool,
                free_tx: resources.free_tx.sender(),
                ready_tx: resources.ready_tx.receiver(),
                tx_pool: pool,
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
    rx_pool: &'resources RxHandoffPool<FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    free_tx: Receiver<'resources, M, u8, TX_QUEUE_DEPTH>,
    free_tx_return: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    ready_tx: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
    link: &'resources SharedLinkState<M>,
    station_address: [u8; 6],
    reserved_tx: Option<u8>,
    tx_reservation: (),
    ingress_fairness: IngressPollFairness,
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
        let lease = self.tx_pool.claim_network(index);
        PinnedTransmitToken {
            free_tx: self.free_tx_return,
            ready_tx: self.ready_tx,
            lease: Some(lease),
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
        let ingress_epoch_capacity = RX_QUEUE_DEPTH.min(TX_QUEUE_DEPTH);
        if !self.ingress_fairness.admit(cx, ingress_epoch_capacity) {
            return None;
        }
        if !self.poll_reserve_tx(cx) {
            self.ingress_fairness
                .record_natural_stop(ingress_epoch_capacity);
            return None;
        }
        let index = match self.ready_rx.poll_receive(cx) {
            Poll::Ready(index) => index,
            Poll::Pending => {
                self.ingress_fairness
                    .record_natural_stop(ingress_epoch_capacity);
                return None;
            }
        };
        self.ingress_fairness.record_received();
        let lease = self.rx_pool.claim_network(index);
        Some((
            PinnedReceiveToken {
                free_rx: self.free_rx,
                lease: Some(lease),
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
        if let Err(TrySendError::Full(_)) = self.ready_rx.try_send(index) {
            unreachable!("one ready entry exists per non-free pinned RX slot");
        }
        result
    }

    pub fn try_send(&mut self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        Self::validate_length(frame.len()).map_err(RxEnqueueError::InvalidLength)?;
        let lease = self.try_claim_slot()?;
        self.publish(lease, frame.len(), |storage| storage.copy_from_slice(frame));
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
        if let Err(TrySendError::Full(_)) = self.ready_rx.try_send(index) {
            unreachable!("one ready entry exists per non-free pinned RX slot");
        }
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
    rx_pool: &'resources RxHandoffPool<FRAME_CAPACITY, RX_QUEUE_DEPTH>,
    free_tx: Sender<'resources, M, u8, TX_QUEUE_DEPTH>,
    ready_tx: Receiver<'resources, M, u8, TX_QUEUE_DEPTH>,
    tx_pool: &'resources PinnedTxPool<FRAME_CAPACITY, HEADROOM, TRAILER, TX_QUEUE_DEPTH>,
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
            rx_pool: self.rx_pool,
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
