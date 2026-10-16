//! Retain an owner while its affine name travels through an executor.

use core::{
    cell::UnsafeCell,
    mem::MaybeUninit,
    sync::atomic::{AtomicBool, Ordering},
};

use crate::UniqueQueueIndex;

/// Construction authority for one array of retained owners.
///
/// # Safety
/// The authority is affine and binds exactly one storage instance to its
/// private name domain. Names must not be usable with another instance,
/// including an instance constructed from another authority of the same
/// Rust type. A sealed interface must keep both the names and the raw
/// storage private and route replies back to their originating instance.
/// Each name also meets [`UniqueQueueIndex`]'s bounds, uniqueness and reuse
/// ordering. A new authority must not recreate names still in flight.
///
/// The domain may carry a short installation brand while `Name` contains
/// only an index. That requires the sealed binding's affine executor
/// endpoints to establish the association; erasing a brand alone is not
/// a proof. Neither a name nor its returned wrapper may reference the
/// retained values or their containing allocation.
#[allow(unsafe_code, reason = "the binding proves the private name domain")]
pub unsafe trait CustodyDomain<const N: usize> {
    type Name: UniqueQueueIndex<N>;
}

/// An initialized cell's unique name, returned only after insertion.
///
/// This value contains no pointer to the retained owner and no lifetime
/// borrowed from it. Moving it to an executor does not move that owner.
/// Dropping or forgetting the name does not remove the retained value.
#[must_use]
pub struct RetainedName<K, const N: usize> {
    name: K,
}

#[allow(unsafe_code, reason = "retention preserves the same affine index")]
// SAFETY: the private constructor consumes the unique input name. It is
// returned only after the cell is initialized, and extraction consumes this
// wrapper. No clone, index mutation or independent constructor is exposed.
unsafe impl<K: UniqueQueueIndex<N>, const N: usize> UniqueQueueIndex<N> for RetainedName<K, N> {
    fn queue_index(&self) -> usize {
        self.name.queue_index()
    }
}

/// One retained value per affine place, without an occupancy check or wait.
///
/// [`Self::insert`] initializes the place and produces its only removal
/// name. [`Self::take`] consumes that exact name after it returns through
/// the binding's reply path. The domain, not a runtime lookup or flag,
/// proves that the chosen cell belongs to this storage and is in the
/// required state. The initialization bits serve only destructor accounting.
///
/// Destruction drops remaining values. An installation which cannot prove
/// that their hardware use has ended must retain this entire store in its
/// shared quarantine owner without running its destructor. The store does
/// not stop hardware or give an executor access to borrowed context.
pub struct IndexedCustody<T, D: CustodyDomain<N>, const N: usize> {
    values: [UnsafeCell<MaybeUninit<T>>; N],
    initialized: [AtomicBool; N],
    _domain: D,
}

#[allow(unsafe_code, reason = "affine names serialize each retained cell")]
// SAFETY: insertion consumes the unique vacant name, and only the resulting
// RetainedName permits access or removal. A mutable borrow of that name
// excludes other access to its cell. The domain binds names to this instance
// and orders reuse across cores. T: Send permits that ownership transfer;
// D: Sync permits sharing the retained construction authority. Exclusive
// destruction cannot race another method's storage borrow.
unsafe impl<T: Send, D: CustodyDomain<N> + Sync, const N: usize> Sync for IndexedCustody<T, D, N> {}

impl<T, D: CustodyDomain<N>, const N: usize> IndexedCustody<T, D, N> {
    pub const fn new(domain: D) -> Self {
        Self {
            values: [const { UnsafeCell::new(MaybeUninit::uninit()) }; N],
            initialized: [const { AtomicBool::new(false) }; N],
            _domain: domain,
        }
    }

    /// Retain the entire owner before publishing its name to an executor.
    pub fn insert(&self, name: D::Name, value: T) -> RetainedName<D::Name, N> {
        let index = name.queue_index();
        #[allow(unsafe_code, reason = "the domain owns this vacant bounded cell")]
        // SAFETY: the consumed name proves index < N and exclusive vacant
        // ownership. No RetainedName for the previous value remains. There
        // is no user code between initialization and destructor accounting.
        unsafe {
            (*self.values.get_unchecked(index).get()).write(value);
            self.initialized
                .get_unchecked(index)
                .store(true, Ordering::Relaxed);
        }
        RetainedName { name }
    }

    /// Execute a finite operation on the retained record without moving it.
    /// The closure cannot retain its borrow beyond the mutable name loan.
    pub fn with_mut<R>(
        &self,
        name: &mut RetainedName<D::Name, N>,
        apply: impl FnOnce(&mut T) -> R,
    ) -> R {
        let index = name.queue_index();
        #[allow(unsafe_code, reason = "the affine initialized name owns this cell")]
        // SAFETY: insertion is the only constructor of RetainedName. Domain
        // association proves this exact initialized cell; &mut name prevents
        // another access or removal until the closure's borrow has ended.
        apply(unsafe { (*self.values.get_unchecked(index).get()).assume_init_mut() })
    }

    /// Recover the original name and owner after the reply returns the name.
    pub fn take(&self, name: RetainedName<D::Name, N>) -> (D::Name, T) {
        let index = name.queue_index();
        #[allow(unsafe_code, reason = "consume the sole initialized-cell name")]
        // SAFETY: the domain binds this name to this initialized cell. Moving
        // the owner out and clearing destructor accounting finishes before
        // the original permission can return and authorize another write.
        let value = unsafe {
            let value = (*self.values.get_unchecked(index).get()).assume_init_read();
            self.initialized
                .get_unchecked(index)
                .store(false, Ordering::Relaxed);
            value
        };
        (name.name, value)
    }
}

impl<T, D: CustodyDomain<N>, const N: usize> Drop for IndexedCustody<T, D, N> {
    fn drop(&mut self) {
        for (cell, initialized) in self.values.iter_mut().zip(&mut self.initialized) {
            if *initialized.get_mut() {
                #[allow(
                    unsafe_code,
                    reason = "exclusive destruction accounts for retained values"
                )]
                // SAFETY: the accounting bit is set immediately after insert
                // and cleared immediately after take, with no intervening
                // user code. Exclusive destruction excludes cell accesses.
                unsafe {
                    cell.get_mut().assume_init_drop()
                };
            }
        }
    }
}

#[cfg(test)]
mod tests;
