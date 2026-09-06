#![no_std]
#![forbid(unsafe_code)]

//! Bounded packet-owner queues implementing the original Xarxa driver API.
//!
//! TX retains the stack's exact packet until the radio builds its physical
//! frame. RX copies into the upstream global pool. That pool has no release
//! notification: allocation failure is an explicit receive drop, never an
//! asynchronous wait for a notification the upstream API cannot deliver.

use core::sync::atomic::{AtomicU32, Ordering};
use core::task::{Context, Poll};

use embassy_sync::blocking_mutex::raw::RawMutex;
use embassy_sync::channel::{Channel, TrySendError};
use embassy_sync::signal::Signal;
use embassy_sync::waitqueue::GenericAtomicWaker;
pub use open_esp_radio_network::{FrameLengthError, LinkState, NetworkInterfaceId, RxEnqueueError};
pub use xarxa_driver as driver;
use xarxa_driver::{Driver, PacketBuf};

mod device;
pub use device::Device;

struct QueuedPacket {
    epoch: u32,
    packet: PacketBuf,
}

/// Queue metadata for a permanent logical interface; packet bytes belong to
/// the upstream global pool. The unique device is created by an exclusive borrow.
pub struct Resources<M: RawMutex, const RX: usize, const TX: usize> {
    split: bool,
    rx_pool_drops: AtomicU32,
    rx: Channel<M, QueuedPacket, RX>,
    tx: Channel<M, QueuedPacket, TX>,
    // Bit 0 is Up; the upper bits identify the association lifetime.
    link: AtomicU32,
    network_waker: GenericAtomicWaker<M>,
    radio_waker: GenericAtomicWaker<M>,
    published: Signal<M, ()>,
}

impl<M: RawMutex, const RX: usize, const TX: usize> Resources<M, RX, TX> {
    pub const fn new() -> Self {
        Self {
            split: false,
            rx_pool_drops: AtomicU32::new(0),
            rx: Channel::new(),
            tx: Channel::new(),
            link: AtomicU32::new(0),
            network_waker: GenericAtomicWaker::new(M::INIT),
            radio_waker: GenericAtomicWaker::new(M::INIT),
            published: Signal::new(),
        }
    }

    pub fn split(
        &mut self,
        interface: NetworkInterfaceId,
        address: [u8; 6],
    ) -> (Device<'_, M, RX, TX>, Endpoint<'_, M, RX, TX>) {
        assert!(RX != 0 && TX != 0, "network queues must not be empty");
        assert!(!self.split, "a permanent endpoint may only be split once");
        self.split = true;
        let endpoint = Endpoint {
            resources: self,
            interface,
        };
        (Device::new(endpoint, address), endpoint)
    }

    fn epoch(&self) -> u32 {
        self.link.load(Ordering::Acquire)
    }
}

impl<M: RawMutex, const RX: usize, const TX: usize> Default for Resources<M, RX, TX> {
    fn default() -> Self {
        Self::new()
    }
}

/// Radio-side queue access. Link authority and RX publication can be narrowed
/// independently, while the scheduler remains the sole TX consumer.
pub struct Endpoint<'a, M: RawMutex, const RX: usize, const TX: usize> {
    resources: &'a Resources<M, RX, TX>,
    interface: NetworkInterfaceId,
}

impl<M: RawMutex, const RX: usize, const TX: usize> Copy for Endpoint<'_, M, RX, TX> {}
impl<M: RawMutex, const RX: usize, const TX: usize> Clone for Endpoint<'_, M, RX, TX> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<'a, M: RawMutex, const RX: usize, const TX: usize> Endpoint<'a, M, RX, TX> {
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }

    pub fn link_controller(&self) -> LinkController<'a, M, RX, TX> {
        LinkController { endpoint: *self }
    }

    pub fn rx_publisher(&self) -> RxPublisher<'a, M, RX, TX> {
        RxPublisher { endpoint: *self }
    }

    pub fn tx_queue_len(&self) -> usize {
        self.resources.tx.len()
    }

    /// RX allocation refusals in the upstream global pool, wrapping at u32::MAX.
    pub fn rx_pool_drops(&self) -> u32 {
        self.resources.rx_pool_drops.load(Ordering::Relaxed)
    }

    pub fn try_receive_tx(&self) -> Option<TxFrame<'a, M>> {
        while let Ok(queued) = self.resources.tx.try_receive() {
            self.resources.network_waker.wake();
            let epoch = self.resources.epoch();
            if epoch & 1 != 0 && epoch == queued.epoch {
                return Some(TxFrame {
                    interface: self.interface,
                    packet: Some(queued.packet),
                    released: &self.resources.network_waker,
                });
            }
        }
        None
    }

    pub async fn receive_tx(&self) -> TxFrame<'a, M> {
        loop {
            if let Some(frame) = self.try_receive_tx() {
                return frame;
            }
            self.resources.tx.ready_to_receive().await;
        }
    }

    pub async fn wait_tx_queue_len_at_least(&self, minimum: usize) {
        while self.tx_queue_len() < minimum {
            self.resources.published.wait().await;
        }
    }

    pub async fn wait_tx_publication(&self) {
        if self.resources.tx.is_empty() {
            self.resources.published.wait().await;
        }
    }
}

/// Exact upstream TX owner, tagged by the interface which accepted it.
pub struct TxFrame<'a, M: RawMutex> {
    interface: NetworkInterfaceId,
    packet: Option<PacketBuf>,
    released: &'a GenericAtomicWaker<M>,
}

impl<M: RawMutex> TxFrame<'_, M> {
    pub const fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }
    pub fn ethernet(&self) -> &[u8] {
        self.packet.as_deref().expect("TX owner is live until drop")
    }
}

impl<M: RawMutex> open_esp_radio_wifi_datapath::SoftwareTxFrame for TxFrame<'_, M> {
    fn interface(&self) -> NetworkInterfaceId {
        self.interface
    }
    fn ethernet(&self) -> &[u8] {
        self.packet.as_deref().expect("TX owner is live until drop")
    }
}

// Queue capacity and packet-pool capacity are separate credits. The other
// core may consume the queue notification while this packet still occupies
// the global pool, so notify again after the owner actually releases its slot.
impl<M: RawMutex> Drop for TxFrame<'_, M> {
    fn drop(&mut self) {
        drop(self.packet.take());
        self.released.wake();
    }
}

/// Link-only authority. Down releases queued owners; already selected TX
/// requests remain owned by the radio's ordinary cancellation machinery.
pub struct LinkController<'a, M: RawMutex, const RX: usize, const TX: usize> {
    endpoint: Endpoint<'a, M, RX, TX>,
}
impl<M: RawMutex, const RX: usize, const TX: usize> Copy for LinkController<'_, M, RX, TX> {}
impl<M: RawMutex, const RX: usize, const TX: usize> Clone for LinkController<'_, M, RX, TX> {
    fn clone(&self) -> Self {
        *self
    }
}
impl<M: RawMutex, const RX: usize, const TX: usize> LinkController<'_, M, RX, TX> {
    pub fn interface(&self) -> NetworkInterfaceId {
        self.endpoint.interface
    }

    /// Called by the one radio role owner; producers may run concurrently.
    pub fn set_link_state(&self, state: LinkState) {
        let resources = self.endpoint.resources;
        let up = state == LinkState::Up;
        let previous = resources.epoch();
        if (previous & 1 != 0) == up {
            return;
        }
        let next = if up {
            previous.wrapping_add(2) | 1
        } else {
            previous & !1
        };
        resources.link.store(next, Ordering::Release);
        if !up {
            resources.rx.clear();
            resources.tx.clear();
        }
        resources.network_waker.wake();
        resources.radio_waker.wake();
        resources.published.signal(());
    }
}

/// RX authority with bounded queue backpressure and fallible pool admission.
pub struct RxPublisher<'a, M: RawMutex, const RX: usize, const TX: usize> {
    endpoint: Endpoint<'a, M, RX, TX>,
}
impl<M: RawMutex, const RX: usize, const TX: usize> Copy for RxPublisher<'_, M, RX, TX> {}
impl<M: RawMutex, const RX: usize, const TX: usize> Clone for RxPublisher<'_, M, RX, TX> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<M: RawMutex, const RX: usize, const TX: usize> RxPublisher<'_, M, RX, TX> {
    pub fn queue_len(&self) -> usize {
        self.endpoint.resources.rx.len()
    }

    /// Readiness covers the endpoint queue, not allocation in the upstream
    /// pool. A later allocation failure returns PoolExhausted to RX accounting.
    pub fn poll_ready(&self, cx: &mut Context<'_>) -> Poll<()> {
        let resources = self.endpoint.resources;
        resources.radio_waker.register(cx.waker());
        if resources.epoch() & 1 == 0 {
            return Poll::Pending;
        }
        resources.rx.poll_ready_to_send(cx)
    }

    fn send_with(&self, len: usize, fill: impl FnOnce(&mut [u8])) -> Result<(), RxEnqueueError> {
        let resources = self.endpoint.resources;
        let epoch = resources.epoch();
        if epoch & 1 == 0 {
            return Err(RxEnqueueError::LinkDown);
        }
        if len < 14 {
            return Err(RxEnqueueError::InvalidLength(FrameLengthError::TooShort));
        }
        if len > driver::Capabilities::default().max_transmission_unit {
            return Err(RxEnqueueError::InvalidLength(FrameLengthError::TooLong));
        }
        if resources.rx.is_full() {
            return Err(RxEnqueueError::QueueFull);
        }
        let mut packet = PacketBuf::try_new().ok_or_else(|| {
            resources.rx_pool_drops.fetch_add(1, Ordering::Relaxed);
            RxEnqueueError::PoolExhausted
        })?;
        packet.set_len(len);
        fill(&mut packet);
        resources
            .rx
            .try_send(QueuedPacket { epoch, packet })
            .map_err(|TrySendError::Full(_)| RxEnqueueError::QueueFull)?;
        resources.network_waker.wake();
        Ok(())
    }

    pub fn try_send(&self, frame: &[u8]) -> Result<(), RxEnqueueError> {
        self.try_send_observed(frame, &mut || {})
    }

    /// Observe a constructed owner before the sole radio producer publishes
    /// it. Allocation and length failures never report an admitted packet.
    pub fn try_send_observed(
        &self,
        frame: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        self.send_with(frame.len(), |destination| {
            destination.copy_from_slice(frame);
            before_publish();
        })
    }

    pub fn try_send_parts(
        &self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
    ) -> Result<(), RxEnqueueError> {
        self.try_send_parts_observed(destination, source, ether_type, payload, &mut || {})
    }

    pub fn try_send_parts_observed(
        &self,
        destination: [u8; 6],
        source: [u8; 6],
        ether_type: u16,
        payload: &[u8],
        before_publish: &mut dyn FnMut(),
    ) -> Result<(), RxEnqueueError> {
        let len = 14usize
            .checked_add(payload.len())
            .ok_or(RxEnqueueError::InvalidLength(FrameLengthError::TooLong))?;
        self.send_with(len, |frame| {
            frame[..6].copy_from_slice(&destination);
            frame[6..12].copy_from_slice(&source);
            frame[12..14].copy_from_slice(&ether_type.to_be_bytes());
            frame[14..].copy_from_slice(payload);
            before_publish();
        })
    }
}

#[cfg(test)]
mod tests;
