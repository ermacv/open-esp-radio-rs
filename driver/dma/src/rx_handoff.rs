//! Bounded RX storage whose state-specific leases hand frames between tasks.

use core::{
    cell::UnsafeCell,
    sync::atomic::{AtomicU8, AtomicUsize, Ordering},
};

const SLOT_FREE: u8 = 0;
const SLOT_NETWORK: u8 = 1;
const SLOT_READY: u8 = 2;
const SLOT_RADIO: u8 = 3;

#[repr(C, align(16))]
struct RxHandoffSlot<const FRAME_CAPACITY: usize> {
    bytes: UnsafeCell<[u8; FRAME_CAPACITY]>,
    length: AtomicUsize,
    state: AtomicU8,
}

impl<const FRAME_CAPACITY: usize> RxHandoffSlot<FRAME_CAPACITY> {
    const fn new() -> Self {
        Self {
            bytes: UnsafeCell::new([0; FRAME_CAPACITY]),
            length: AtomicUsize::new(0),
            state: AtomicU8::new(SLOT_FREE),
        }
    }

    fn claim(&self, from: u8, to: u8, message: &str) {
        assert_eq!(
            self.state
                .compare_exchange(from, to, Ordering::AcqRel, Ordering::Acquire),
            Ok(from),
            "{message}"
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
            "only the radio lease may publish an RX handoff slot"
        );
    }

    fn release(&self, owner: u8, message: &str) {
        self.length.store(0, Ordering::Relaxed);
        self.claim(owner, SLOT_FREE, message);
    }

    fn length(&self) -> usize {
        self.length.load(Ordering::Acquire)
    }

    fn storage_mut_ptr(&self) -> *mut u8 {
        self.bytes.get().cast::<u8>()
    }
}

// SAFETY: the atomic state machine and non-Clone leases allow exactly one
// stage to access the UnsafeCell. Release/acquire transitions publish bytes
// and length before ownership is handed to the next stage.
#[allow(unsafe_code, reason = "RX handoff state machine is its Sync boundary")]
unsafe impl<const FRAME_CAPACITY: usize> Sync for RxHandoffSlot<FRAME_CAPACITY> {}

/// Fixed storage for copy-minimal radio-to-network frame handoff.
///
/// The pool itself need not be exposed to either task. They receive
/// state-specific leases that cannot perform another stage's transition.
pub struct RxHandoffPool<const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> {
    slots: [RxHandoffSlot<FRAME_CAPACITY>; QUEUE_DEPTH],
}

impl<const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize>
    RxHandoffPool<FRAME_CAPACITY, QUEUE_DEPTH>
{
    pub const fn new() -> Self {
        Self {
            slots: [const { RxHandoffSlot::new() }; QUEUE_DEPTH],
        }
    }

    pub fn claim_radio(&self, index: u8) -> RxRadioLease<'_, FRAME_CAPACITY> {
        let slot = self
            .slots
            .get(usize::from(index))
            .expect("RX handoff index belongs to this pool");
        slot.claim(
            SLOT_FREE,
            SLOT_RADIO,
            "free-channel entry did not name a free RX handoff slot",
        );
        RxRadioLease {
            slot,
            index,
            live: true,
        }
    }

    pub fn claim_network(&self, index: u8) -> RxNetworkLease<'_, FRAME_CAPACITY> {
        let slot = self
            .slots
            .get(usize::from(index))
            .expect("RX handoff index belongs to this pool");
        slot.claim(
            SLOT_READY,
            SLOT_NETWORK,
            "ready-channel entry did not name a ready RX handoff slot",
        );
        RxNetworkLease {
            slot,
            index,
            live: true,
        }
    }
}

impl<const FRAME_CAPACITY: usize, const QUEUE_DEPTH: usize> Default
    for RxHandoffPool<FRAME_CAPACITY, QUEUE_DEPTH>
{
    fn default() -> Self {
        Self::new()
    }
}

/// Unique radio writer for one free RX handoff slot.
pub struct RxRadioLease<'pool, const FRAME_CAPACITY: usize> {
    slot: &'pool RxHandoffSlot<FRAME_CAPACITY>,
    index: u8,
    live: bool,
}

impl<const FRAME_CAPACITY: usize> RxRadioLease<'_, FRAME_CAPACITY> {
    pub fn publish<R>(mut self, length: usize, write: impl FnOnce(&mut [u8]) -> R) -> (u8, R) {
        assert!(length <= FRAME_CAPACITY, "RX frame exceeds slot capacity");
        // SAFETY: this non-Clone Radio lease is the unique SLOT_RADIO owner.
        // The Ready transition happens only after the writer returns.
        #[allow(unsafe_code, reason = "radio lease uniquely initializes its RX slot")]
        let frame = unsafe { core::slice::from_raw_parts_mut(self.slot.storage_mut_ptr(), length) };
        let result = write(frame);
        self.slot.publish_ready(length);
        self.live = false;
        (self.index, result)
    }

    pub fn release(mut self) -> u8 {
        self.slot.release(
            SLOT_RADIO,
            "only an unpublished radio lease may return an RX handoff slot",
        );
        self.live = false;
        self.index
    }
}

impl<const FRAME_CAPACITY: usize> Drop for RxRadioLease<'_, FRAME_CAPACITY> {
    fn drop(&mut self) {
        if self.live {
            self.slot.release(
                SLOT_RADIO,
                "only the live radio lease may return an RX handoff slot",
            );
        }
    }
}

/// Unique network reader for one ready RX handoff slot.
pub struct RxNetworkLease<'pool, const FRAME_CAPACITY: usize> {
    slot: &'pool RxHandoffSlot<FRAME_CAPACITY>,
    index: u8,
    live: bool,
}

impl<const FRAME_CAPACITY: usize> RxNetworkLease<'_, FRAME_CAPACITY> {
    pub fn with_frame<R>(&mut self, read: impl FnOnce(&mut [u8]) -> R) -> R {
        let length = self.slot.length();
        // SAFETY: `&mut self` borrows the unique non-Clone SLOT_NETWORK lease.
        #[allow(unsafe_code, reason = "network lease uniquely owns its RX slot")]
        let frame = unsafe { core::slice::from_raw_parts_mut(self.slot.storage_mut_ptr(), length) };
        read(frame)
    }

    pub fn release(mut self) -> u8 {
        self.slot.release(
            SLOT_NETWORK,
            "only the network lease may return an RX handoff slot",
        );
        self.live = false;
        self.index
    }
}

impl<const FRAME_CAPACITY: usize> Drop for RxNetworkLease<'_, FRAME_CAPACITY> {
    fn drop(&mut self) {
        if self.live {
            self.slot.release(
                SLOT_NETWORK,
                "only the live network lease may return an RX handoff slot",
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::RxHandoffPool;

    #[test]
    fn one_address_crosses_radio_and_network_ownership() {
        let pool = RxHandoffPool::<32, 1>::new();
        let radio = pool.claim_radio(0);
        let (index, radio_address) = radio.publish(4, |frame| {
            frame.copy_from_slice(&[1, 2, 3, 4]);
            frame.as_ptr() as usize
        });

        let mut network = pool.claim_network(index);
        let network_address = network.with_frame(|frame| {
            assert_eq!(frame, &[1, 2, 3, 4]);
            frame.as_ptr() as usize
        });
        assert_eq!(radio_address, network_address);
        assert_eq!(network.release(), 0);
    }

    #[test]
    fn dropped_leases_restore_their_slot_state() {
        let pool = RxHandoffPool::<16, 1>::new();
        drop(pool.claim_radio(0));

        let radio = pool.claim_radio(0);
        let (index, ()) = radio.publish(1, |frame| frame[0] = 7);
        drop(pool.claim_network(index));

        let radio = pool.claim_radio(0);
        let (index, ()) = radio.publish(1, |frame| frame[0] = 9);
        assert_eq!(pool.claim_network(index).release(), 0);
    }
}
