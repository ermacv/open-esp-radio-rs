//! Fixed owned application-data transmit channel.

use core::{
    cell::UnsafeCell,
    future::poll_fn,
    sync::atomic::{AtomicUsize, Ordering},
    task::{Context, Poll},
};

use crate::{channel::BoundedChannel, data_rx::WifiDataInterface, queue::WakerCell};

pub const WIFI_DATA_TX_CAPACITY: usize = 32;
pub const WIFI_DATA_TX_FRAME_CAPACITY: usize = 1600;
const ETHERNET_HEADER_LEN: usize = 14;
const HARDWARE_CREDIT_FREE: usize = 0;
const HARDWARE_CREDIT_RESERVED: usize = 1;

struct TxSlotData {
    interface: WifiDataInterface,
    length: usize,
    bytes: [u8; WIFI_DATA_TX_FRAME_CAPACITY],
}

struct TxSlot {
    // Zero is free, one is reserved by an owned application frame, and any
    // other value is the exact vendor ESF frame awaiting hardware completion.
    hardware_frame: AtomicUsize,
    data: UnsafeCell<TxSlotData>,
}

impl TxSlot {
    const fn new() -> Self {
        Self {
            hardware_frame: AtomicUsize::new(HARDWARE_CREDIT_FREE),
            data: UnsafeCell::new(TxSlotData {
                interface: WifiDataInterface::Station,
                length: 0,
                bytes: [0; WIFI_DATA_TX_FRAME_CAPACITY],
            }),
        }
    }
}

unsafe impl Sync for TxSlot {}

#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.data_tx_slots"
)]
static TX_SLOTS: [TxSlot; WIFI_DATA_TX_CAPACITY] = [const { TxSlot::new() }; WIFI_DATA_TX_CAPACITY];
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.data_tx_channel"
)]
static TX_CHANNEL: BoundedChannel<TxSlotToken, WIFI_DATA_TX_CAPACITY> = BoundedChannel::new();
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.data_tx_wakers"
)]
static TX_CAPACITY_WAKER: WakerCell = WakerCell::new();
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".critical.bss.wifi_strict.data_tx_wakers"
)]
static TX_IDLE_WAKER: WakerCell = WakerCell::new();
static TX_CLAIMED: AtomicUsize = AtomicUsize::new(0);
static TX_ENQUEUED: AtomicUsize = AtomicUsize::new(0);
static TX_DEQUEUED: AtomicUsize = AtomicUsize::new(0);
static TX_RELEASED: AtomicUsize = AtomicUsize::new(0);
static TX_REJECTED_INVALID: AtomicUsize = AtomicUsize::new(0);
static TX_REJECTED_SLOTS_FULL: AtomicUsize = AtomicUsize::new(0);
static TX_REJECTED_CHANNEL_CONTENDED: AtomicUsize = AtomicUsize::new(0);
static TX_REJECTED_HARDWARE_CREDIT: AtomicUsize = AtomicUsize::new(0);
static TX_REJECTED_PEER_MISSING: AtomicUsize = AtomicUsize::new(0);
static TX_LAST_MISSING_PEER_ERROR: AtomicUsize = AtomicUsize::new(0);
static TX_OCCUPIED: AtomicUsize = AtomicUsize::new(0);
static TX_OCCUPIED_HIGH_WATER: AtomicUsize = AtomicUsize::new(0);
static TX_HARDWARE_COMMITTED: AtomicUsize = AtomicUsize::new(0);
static TX_HARDWARE_RELEASED: AtomicUsize = AtomicUsize::new(0);

fn record_high_water(counter: &AtomicUsize, value: usize) {
    let observed = counter.load(Ordering::Relaxed);
    if value > observed {
        // Diagnostics must preserve the runtime's wait-free producer
        // contract. A racing update may conservatively win; never retry.
        let _ = counter.compare_exchange(observed, value, Ordering::Relaxed, Ordering::Relaxed);
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WifiDataTxSnapshot {
    pub claimed: usize,
    pub enqueued: usize,
    pub dequeued: usize,
    pub released: usize,
    pub rejected_invalid: usize,
    pub rejected_slots_full: usize,
    pub rejected_channel_contended: usize,
    pub rejected_hardware_credit: usize,
    pub rejected_peer_missing: usize,
    pub last_missing_peer_error: usize,
    pub occupied: usize,
    pub occupied_high_water: usize,
    pub queued: usize,
    pub hardware_credit_in_use: bool,
    pub hardware_credits_in_use: usize,
    pub hardware_credit_capacity: usize,
    pub hardware_committed: usize,
    pub hardware_released: usize,
}

pub fn wifi_data_tx_snapshot() -> WifiDataTxSnapshot {
    let hardware_credits_in_use = TX_SLOTS
        .iter()
        .filter(|slot| slot.hardware_frame.load(Ordering::Acquire) != HARDWARE_CREDIT_FREE)
        .count();
    WifiDataTxSnapshot {
        claimed: TX_CLAIMED.load(Ordering::Acquire),
        enqueued: TX_ENQUEUED.load(Ordering::Acquire),
        dequeued: TX_DEQUEUED.load(Ordering::Acquire),
        released: TX_RELEASED.load(Ordering::Acquire),
        rejected_invalid: TX_REJECTED_INVALID.load(Ordering::Acquire),
        rejected_slots_full: TX_REJECTED_SLOTS_FULL.load(Ordering::Acquire),
        rejected_channel_contended: TX_REJECTED_CHANNEL_CONTENDED.load(Ordering::Acquire),
        rejected_hardware_credit: TX_REJECTED_HARDWARE_CREDIT.load(Ordering::Acquire),
        rejected_peer_missing: TX_REJECTED_PEER_MISSING.load(Ordering::Acquire),
        last_missing_peer_error: TX_LAST_MISSING_PEER_ERROR.load(Ordering::Acquire),
        occupied: TX_OCCUPIED.load(Ordering::Acquire),
        occupied_high_water: TX_OCCUPIED_HIGH_WATER.load(Ordering::Acquire),
        queued: TX_CHANNEL.len(),
        hardware_credit_in_use: hardware_credits_in_use != 0,
        hardware_credits_in_use,
        hardware_credit_capacity: WIFI_DATA_TX_CAPACITY,
        hardware_committed: TX_HARDWARE_COMMITTED.load(Ordering::Acquire),
        hardware_released: TX_HARDWARE_RELEASED.load(Ordering::Acquire),
    }
}

/// Reject one already-owned network frame whose peer disappeared before the
/// radio owner reached it.
///
/// This is a normal bounded link-transition race, not a reason to terminate
/// the immortal radio executor. Dropping the command after this call returns
/// its reserved static TX slot through `TxSlotToken::drop`.
pub(crate) fn reject_wifi_data_tx_missing_peer(peer_error: u32) {
    TX_LAST_MISSING_PEER_ERROR.store(peer_error as usize, Ordering::Release);
    TX_REJECTED_PEER_MISSING.fetch_add(1, Ordering::Relaxed);
}

struct TxSlotToken {
    index: usize,
}

impl Drop for TxSlotToken {
    fn drop(&mut self) {
        // Before the vendor frame is committed, dropping ownership cancels
        // this slot's reservation. Once committed, only the matching TX-done
        // frame releases both the copy storage and hardware credit. A very
        // fast TX completion may already have released the slot before the
        // owned Rust frame is dropped, in which case this is intentionally a
        // no-op.
        if TX_SLOTS[self.index]
            .hardware_frame
            .compare_exchange(
                HARDWARE_CREDIT_RESERVED,
                HARDWARE_CREDIT_FREE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
        {
            release_slot();
        }
    }
}

pub struct OwnedWifiDataTxFrame {
    token: TxSlotToken,
}

impl OwnedWifiDataTxFrame {
    pub fn interface(&self) -> WifiDataInterface {
        unsafe { (*TX_SLOTS[self.token.index].data.get()).interface }
    }

    pub fn as_bytes(&self) -> &[u8] {
        let data = unsafe { &*TX_SLOTS[self.token.index].data.get() };
        &data.bytes[..data.length]
    }

    pub fn destination(&self) -> &[u8; 6] {
        self.as_bytes()[..6].try_into().unwrap()
    }

    pub(crate) fn commit_hardware_credit(&self, frame: *mut u8) -> Result<(), ()> {
        let address = frame as usize;
        if address <= HARDWARE_CREDIT_RESERVED {
            return Err(());
        }
        TX_SLOTS[self.token.index]
            .hardware_frame
            .compare_exchange(
                HARDWARE_CREDIT_RESERVED,
                address,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .map_err(|_| ())?;
        TX_HARDWARE_COMMITTED.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WifiDataTxEnqueueError {
    InvalidLength,
    SlotsFull,
    ChannelContended,
    HardwareCreditUnavailable,
}

/// Copy one complete Ethernet frame into fixed storage without waiting.
pub fn try_send_wifi_data(
    interface: WifiDataInterface,
    frame: &[u8],
) -> Result<(), WifiDataTxEnqueueError> {
    if frame.len() < ETHERNET_HEADER_LEN || frame.len() > WIFI_DATA_TX_FRAME_CAPACITY {
        TX_REJECTED_INVALID.fetch_add(1, Ordering::Relaxed);
        return Err(WifiDataTxEnqueueError::InvalidLength);
    }
    let Some((index, slot)) = TX_SLOTS.iter().enumerate().find(|(_, slot)| {
        slot.hardware_frame
            .compare_exchange(
                HARDWARE_CREDIT_FREE,
                HARDWARE_CREDIT_RESERVED,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }) else {
        TX_REJECTED_HARDWARE_CREDIT.fetch_add(1, Ordering::Relaxed);
        TX_REJECTED_SLOTS_FULL.fetch_add(1, Ordering::Relaxed);
        return Err(WifiDataTxEnqueueError::HardwareCreditUnavailable);
    };

    TX_CLAIMED.fetch_add(1, Ordering::Relaxed);
    let occupied = TX_OCCUPIED.fetch_add(1, Ordering::AcqRel) + 1;
    record_high_water(&TX_OCCUPIED_HIGH_WATER, occupied);

    let data = unsafe { &mut *slot.data.get() };
    data.interface = interface;
    data.length = frame.len();
    data.bytes[..frame.len()].copy_from_slice(frame);
    if let Err(error) = TX_CHANNEL.try_send(TxSlotToken { index }) {
        TX_REJECTED_CHANNEL_CONTENDED.fetch_add(1, Ordering::Relaxed);
        drop(error.0);
        return Err(WifiDataTxEnqueueError::ChannelContended);
    }
    TX_ENQUEUED.fetch_add(1, Ordering::Release);
    Ok(())
}

pub fn try_receive_wifi_data_tx() -> Option<OwnedWifiDataTxFrame> {
    TX_CHANNEL.try_receive().map(|token| {
        TX_DEQUEUED.fetch_add(1, Ordering::Relaxed);
        OwnedWifiDataTxFrame { token }
    })
}

pub async fn receive_wifi_data_tx() -> OwnedWifiDataTxFrame {
    let token = TX_CHANNEL.receive().await;
    TX_DEQUEUED.fetch_add(1, Ordering::Relaxed);
    OwnedWifiDataTxFrame { token }
}

/// Register an executor waker and report whether a fixed TX slot is free.
///
/// The returned readiness is advisory: the caller must still handle the
/// bounded `try_send_wifi_data` result. Dropping a radio-owned frame wakes the
/// registered network driver without a timer or retry loop.
pub fn poll_wifi_data_tx_ready(cx: &mut Context<'_>) -> bool {
    TX_CAPACITY_WAKER.register(cx.waker());
    TX_SLOTS
        .iter()
        .any(|slot| slot.hardware_frame.load(Ordering::Acquire) == HARDWARE_CREDIT_FREE)
        && TX_CHANNEL.len() < WIFI_DATA_TX_CAPACITY
}

/// Await the exact hardware-completion edge for every admitted data frame.
///
/// This is an event-driven flush boundary: completions wake the future through
/// its dedicated completion waker; no timer, status loop, yield, or RTOS primitive is
/// involved.
pub async fn flush_wifi_data_tx() {
    poll_fn(|cx| {
        // Capacity readiness and an exact flush can be awaited concurrently.
        // They cannot share WakerCell's intentionally single registered waker:
        // the network runner would otherwise be able to replace the flush
        // waiter after its last observation and lose the final TX-done edge.
        TX_IDLE_WAKER.register(cx.waker());
        if TX_CHANNEL.is_empty()
            && TX_SLOTS
                .iter()
                .all(|slot| slot.hardware_frame.load(Ordering::Acquire) == HARDWARE_CREDIT_FREE)
        {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    })
    .await
}

fn release_slot() {
    TX_RELEASED.fetch_add(1, Ordering::Relaxed);
    TX_OCCUPIED.fetch_sub(1, Ordering::AcqRel);
    TX_CAPACITY_WAKER.wake();
    TX_IDLE_WAKER.wake();
}

/// Release one data descriptor credit from the matching hardware completion
/// edge. Unrelated management/EAPOL completions are ignored. The bounded scan
/// has no retry/wait edge and preserves exact pointer ownership.
#[cfg_attr(
    target_arch = "riscv32",
    link_section = ".rwtext.wifi_strict.data_tx_done"
)]
pub(crate) fn complete_hardware_wifi_data_tx(frame: *mut u8) -> bool {
    let address = frame as usize;
    if address <= HARDWARE_CREDIT_RESERVED {
        return false;
    }
    let Some(slot) = TX_SLOTS.iter().find(|slot| {
        slot.hardware_frame
            .compare_exchange(
                address,
                HARDWARE_CREDIT_FREE,
                Ordering::AcqRel,
                Ordering::Acquire,
            )
            .is_ok()
    }) else {
        return false;
    };
    let _ = slot;
    TX_HARDWARE_RELEASED.fetch_add(1, Ordering::Relaxed);
    release_slot();
    true
}

/// Return whether `frame` is the exact vendor buffer currently owned by one
/// Rust data-TX slot. This is a bounded diagnostic/dispatch predicate: it
/// neither changes ownership nor waits for a completion.
pub(crate) fn owns_hardware_wifi_data_tx(frame: *mut u8) -> bool {
    let address = frame as usize;
    address > HARDWARE_CREDIT_RESERVED
        && TX_SLOTS
            .iter()
            .any(|slot| slot.hardware_frame.load(Ordering::Acquire) == address)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tx_slot_owns_complete_ethernet_frame() {
        let mut frame = [0_u8; ETHERNET_HEADER_LEN];
        frame[..6].copy_from_slice(&[1, 2, 3, 4, 5, 6]);
        assert_eq!(
            try_send_wifi_data(WifiDataInterface::AccessPoint, &frame),
            Ok(())
        );
        let owned = try_receive_wifi_data_tx().unwrap();
        assert_eq!(owned.interface(), WifiDataInterface::AccessPoint);
        assert_eq!(owned.destination(), &[1, 2, 3, 4, 5, 6]);
        assert_eq!(owned.as_bytes(), &frame);
        let hardware_frame = 0x100usize as *mut u8;
        assert_eq!(owned.commit_hardware_credit(hardware_frame), Ok(()));
        drop(owned);
        assert!(wifi_data_tx_snapshot().hardware_credit_in_use);
        assert!(complete_hardware_wifi_data_tx(hardware_frame));
        assert!(!wifi_data_tx_snapshot().hardware_credit_in_use);
    }

    #[test]
    fn tx_rejects_non_ethernet_and_oversized_frames() {
        assert_eq!(
            try_send_wifi_data(WifiDataInterface::Station, &[0; 13]),
            Err(WifiDataTxEnqueueError::InvalidLength)
        );
        assert_eq!(
            try_send_wifi_data(
                WifiDataInterface::Station,
                &[0; WIFI_DATA_TX_FRAME_CAPACITY + 1],
            ),
            Err(WifiDataTxEnqueueError::InvalidLength)
        );
    }
}
