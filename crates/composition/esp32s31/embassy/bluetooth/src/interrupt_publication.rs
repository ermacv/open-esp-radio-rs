//! Reclaimable ISR publication with an affine route owner.
//!
//! The callback borrows its service only inside the slot's critical section.
//! Successful route shutdown precedes removal; a rejected shutdown returns the
//! complete publication unchanged. Dropping a live publication leaves the slot
//! occupied, because Drop cannot establish hardware quiescence.

use core::cell::RefCell;
use embassy_sync::blocking_mutex::{Mutex, raw::RawMutex};

pub(crate) struct InterruptPublicationSlot<M: RawMutex, T> {
    service: Mutex<M, RefCell<Option<T>>>,
}

impl<M: RawMutex, T> InterruptPublicationSlot<M, T> {
    pub(crate) const fn new() -> Self {
        Self {
            service: Mutex::new(RefCell::new(None)),
        }
    }

    /// No reference into a replaceable publication escapes this callback.
    pub(crate) fn with<R>(&self, visit: impl FnOnce(Option<&T>) -> R) -> R {
        self.service.lock(|slot| visit(slot.borrow().as_ref()))
    }

    /// Publish the whole service before routes can enter. A rejected route
    /// bind must leave routes inactive; the exact service is then returned.
    pub(crate) fn bind<Routes, Error>(
        &self,
        service: T,
        bind: impl FnOnce() -> Result<Routes, Error>,
    ) -> Result<InterruptPublication<'_, M, T, Routes>, (Option<Error>, T)> {
        let rejected = self.service.lock(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.is_some() {
                Some(service)
            } else {
                *slot = Some(service);
                None
            }
        });
        if let Some(service) = rejected {
            return Err((None, service));
        }
        match bind() {
            Ok(routes) => Ok(InterruptPublication { slot: self, routes }),
            Err(error) => Err((Some(error), self.take())),
        }
    }

    fn take(&self) -> T {
        self.service.lock(|slot| {
            slot.borrow_mut()
                .take()
                .expect("the affine publication owns its occupied slot")
        })
    }
}

#[must_use = "retain live routes until explicit successful shutdown"]
pub(crate) struct InterruptPublication<'a, M: RawMutex, T, Routes> {
    slot: &'a InterruptPublicationSlot<M, T>,
    routes: Routes,
}

impl<M: RawMutex, T, Routes> InterruptPublication<'_, M, T, Routes> {
    pub(crate) fn with<R>(&self, visit: impl FnOnce(&T) -> R) -> R {
        self.slot
            .with(|service| visit(service.expect("live publication retains its service")))
    }

    /// The supplied transition must disable the entire route set and reject
    /// wrong-core shutdown before mutation. Removal under the same slot lock
    /// also excludes any callback still borrowing the service.
    pub(crate) fn disable<Error>(
        self,
        disable: impl FnOnce(Routes) -> Result<(), (Error, Routes)>,
    ) -> Result<T, (Error, Self)> {
        let Self { slot, routes } = self;
        match disable(routes) {
            Ok(()) => Ok(slot.take()),
            Err((error, routes)) => Err((error, Self { slot, routes })),
        }
    }
}

#[cfg(test)]
mod tests;
