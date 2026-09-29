//! Admission credit follows a software packet through queueing and radio retention.

use core::{cell::RefCell, task::Waker};

use embassy_sync::{
    blocking_mutex::{Mutex, raw::RawMutex},
    waitqueue::WakerRegistration,
};

struct State {
    outstanding: usize,
    peak_outstanding: usize,
    refused: u32,
    sender: WakerRegistration,
}

/// Snapshot of one endpoint's software TX admission.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TxCreditCounters {
    /// Credits held now by queued and radio-retained owners.
    pub outstanding: usize,
    /// Highest `outstanding` since the last peak restart.
    pub peak_outstanding: usize,
    /// Transmissions refused because every credit was held.
    pub refused: u32,
    /// Credits the endpoint has in total.
    pub capacity: usize,
}

pub(super) struct TxBudget<M: RawMutex> {
    capacity: usize,
    state: Mutex<M, RefCell<State>>,
}

impl<M: RawMutex> TxBudget<M> {
    pub(super) const fn new(capacity: usize) -> Self {
        Self {
            capacity,
            state: Mutex::new(RefCell::new(State {
                outstanding: 0,
                peak_outstanding: 0,
                refused: 0,
                sender: WakerRegistration::new(),
            })),
        }
    }

    pub(super) fn register_sender(&self, waker: &Waker) {
        self.state
            .lock(|state| state.borrow_mut().sender.register(waker));
    }

    pub(super) fn is_full(&self) -> bool {
        self.state
            .lock(|state| state.borrow().outstanding == self.capacity)
    }

    pub(super) fn try_admit(&self) -> bool {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            if state.outstanding == self.capacity {
                state.refused = state.refused.wrapping_add(1);
                return false;
            }
            state.outstanding += 1;
            state.peak_outstanding = state.peak_outstanding.max(state.outstanding);
            true
        })
    }

    pub(super) fn counters(&self) -> TxCreditCounters {
        self.state.lock(|state| {
            let state = state.borrow();
            TxCreditCounters {
                outstanding: state.outstanding,
                peak_outstanding: state.peak_outstanding,
                refused: state.refused,
                capacity: self.capacity,
            }
        })
    }

    /// Start a new peak interval at the current occupancy.
    pub(super) fn restart_peak(&self) {
        self.state.lock(|state| {
            let mut state = state.borrow_mut();
            state.peak_outstanding = state.outstanding;
        })
    }

    /// Transfer a queue's already admitted credit to its radio-side owner.
    pub(super) fn claim(&self) -> TxCredit<'_, M> {
        TxCredit(self)
    }
}

/// Non-cloneable credit; stored after the packet so packet destruction happens
/// before the producer is told that another admission is possible.
pub(super) struct TxCredit<'resources, M: RawMutex>(&'resources TxBudget<M>);

impl<M: RawMutex> Drop for TxCredit<'_, M> {
    fn drop(&mut self) {
        self.0.state.lock(|state| {
            let mut state = state.borrow_mut();
            let was_full = state.outstanding == self.0.capacity;
            state.outstanding -= 1;
            if was_full {
                state.sender.wake();
            }
        });
    }
}
