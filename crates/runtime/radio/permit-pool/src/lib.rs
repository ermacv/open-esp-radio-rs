#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! An allocation-free, first-in, first-out pool of a radio port install's
//! permits ([`oer_radio_port::PermitPool`]).
//!
//! A backend keeps a [`FifoPermitPool`] in its held part of the install
//! (`PortBackend::Held`) and fills it with permits its `resources` mint
//! ([`Mint::permits`](oer_radio_port::Mint::permits)), so every permit in
//! it is real capacity of that install. Takers wait in arrival order, never
//! refused:
//!
//! - a taker gets its place in the line at its first poll, and is served
//!   once it leads the line and a permit is free;
//! - a released permit wakes the leader; a served taker that leaves a
//!   permit free wakes the next leader, so two releases before any taker
//!   runs serve two takers;
//! - a taker whose future drops gives up its place, and the taker behind it
//!   leads.
//!
//! The line holds `WAITERS` places, and `WAITERS` must cover every taker
//! that can wait at once, which the backend knows from its own structure
//! (each interface client's one operation, each queue's transmissions). A
//! taker that finds the line full is a fault of the composition: the pool
//! panics (owner decision on #703), so arrival order is always strict.
//!
//! The pool is `!Sync`: it lives in one task, the port body's, and is
//! shared there by reference between the body's parts. Nothing outside the
//! task reaches it.
//!
//! No caller makes a permit, so no caller adds capacity:
//!
//! ```compile_fail,E0451
//! # use oer_radio_port::Permit;
//! fn forge<'id, K>() -> Permit<'id, K> {
//!     Permit { index: 0, brand: core::marker::PhantomData, capabilities: core::marker::PhantomData }
//! }
//! ```
//!
//! A pool of one install does not take another's permits:
//!
//! ```compile_fail
//! # use oer_radio_permit_pool::FifoPermitPool;
//! # use oer_radio_port::Permit;
//! fn mix<'a, 'b, K>(permit: Permit<'a, K>) -> FifoPermitPool<'b, K, 1, 2> {
//!     FifoPermitPool::new([permit])
//! }
//! ```
//!
//! Its compiling twin fills a pool with its own install's permits:
//!
//! ```
//! # use oer_radio_permit_pool::FifoPermitPool;
//! # use oer_radio_port::Permit;
//! fn fill<'id, K>(permit: Permit<'id, K>) -> FifoPermitPool<'id, K, 1, 2> {
//!     FifoPermitPool::new([permit])
//! }
//! ```
//!
//! The pool is not `Sync`, so no other task shares it:
//!
//! ```compile_fail,E0277
//! # use oer_radio_permit_pool::FifoPermitPool;
//! fn shared<T: Sync>() {}
//! fn check<'id, K>() {
//!     shared::<FifoPermitPool<'id, K, 1, 2>>();
//! }
//! ```
//!
//! ```
//! # use oer_radio_permit_pool::FifoPermitPool;
//! fn owned<T>() {}
//! fn check<'id, K>() {
//!     owned::<FifoPermitPool<'id, K, 1, 2>>();
//! }
//! ```

use core::{
    cell::{Cell, RefCell},
    future::{Future, poll_fn},
    task::{Context, Poll, Waker},
};

use oer_radio_port::{Permit, PermitPool};

/// A first-in, first-out pool of `PERMITS` permits of one install,
/// counting for `K`, with a line of `WAITERS` places.
pub struct FifoPermitPool<'id, K, const PERMITS: usize, const WAITERS: usize> {
    /// The permits not held, at most `PERMITS`.
    free: RefCell<[Option<Permit<'id, K>>; PERMITS]>,
    /// The places in the line: each one's ticket and its taker's waker.
    line: RefCell<[Option<(u32, Waker)>; WAITERS]>,
    /// The next ticket.
    next: Cell<u32>,
}

impl<'id, K, const PERMITS: usize, const WAITERS: usize> FifoPermitPool<'id, K, PERMITS, WAITERS> {
    /// A pool of `permits`, every one free.
    pub fn new(permits: [Permit<'id, K>; PERMITS]) -> Self {
        const { assert!(WAITERS > 0, "a line holds at least one place") };
        Self {
            free: RefCell::new(permits.map(Some)),
            line: RefCell::new([const { None }; WAITERS]),
            next: Cell::new(0),
        }
    }

    /// The permits not held now.
    pub fn available(&self) -> usize {
        self.free.borrow().iter().flatten().count()
    }

    /// The ticket that leads the line.
    fn leader(&self) -> Option<u32> {
        self.line
            .borrow()
            .iter()
            .flatten()
            .map(|(ticket, _)| *ticket)
            .min_by_key(|ticket| ticket.wrapping_sub(self.next.get()))
    }

    /// Wake the leader of the line.
    fn wake_leader(&self) {
        let Some(leader) = self.leader() else {
            return;
        };
        if let Some((_, waker)) = self
            .line
            .borrow()
            .iter()
            .flatten()
            .find(|(ticket, _)| *ticket == leader)
        {
            waker.wake_by_ref();
        }
    }

    /// Give up `ticket`'s place.
    fn leave(&self, ticket: u32) {
        let led = self.leader() == Some(ticket);
        if let Some(place) = self
            .line
            .borrow_mut()
            .iter_mut()
            .find(|place| place.as_ref().is_some_and(|(held, _)| *held == ticket))
        {
            *place = None;
        }
        if led && self.available() > 0 {
            self.wake_leader();
        }
    }

    /// One poll of a taker holding `ticket`, or none yet.
    fn poll_take(
        &self,
        context: &mut Context<'_>,
        ticket: &mut Option<u32>,
    ) -> Poll<Permit<'id, K>> {
        let mine = match *ticket {
            Some(mine) => {
                if let Some((_, waker)) = self
                    .line
                    .borrow_mut()
                    .iter_mut()
                    .flatten()
                    .find(|(held, _)| *held == mine)
                {
                    waker.clone_from(context.waker());
                }
                mine
            }
            None => {
                let mut line = self.line.borrow_mut();
                let Some(place) = line.iter_mut().find(|place| place.is_none()) else {
                    panic!("more takers wait than the pool's line has places (`WAITERS`)");
                };
                let issued = self.next.get();
                self.next.set(issued.wrapping_add(1));
                *place = Some((issued, context.waker().clone()));
                *ticket = Some(issued);
                issued
            }
        };
        if self.leader() != Some(mine) {
            return Poll::Pending;
        }
        let served = self.free.borrow_mut().iter_mut().find_map(Option::take);
        let Some(permit) = served else {
            return Poll::Pending;
        };
        *ticket = None;
        self.leave(mine);
        Poll::Ready(permit)
    }
}

/// A taker's place in a [`FifoPermitPool`]'s line, given up when its future
/// drops before it is served.
struct Place<'p, 'id, K, const PERMITS: usize, const WAITERS: usize> {
    pool: &'p FifoPermitPool<'id, K, PERMITS, WAITERS>,
    ticket: Option<u32>,
}

impl<K, const PERMITS: usize, const WAITERS: usize> Drop for Place<'_, '_, K, PERMITS, WAITERS> {
    fn drop(&mut self) {
        if let Some(ticket) = self.ticket.take() {
            self.pool.leave(ticket);
        }
    }
}

impl<'id, K, const PERMITS: usize, const WAITERS: usize> PermitPool<'id, K>
    for FifoPermitPool<'id, K, PERMITS, WAITERS>
{
    fn take(&self) -> impl Future<Output = Permit<'id, K>> + '_ {
        let mut place = Place {
            pool: self,
            ticket: None,
        };
        poll_fn(move |context| {
            // The whole place moves into the future, so dropping the future
            // drops it.
            let place = &mut place;
            self.poll_take(context, &mut place.ticket)
        })
    }

    fn release(&self, permit: Permit<'id, K>) {
        let mut free = self.free.borrow_mut();
        let slot = free
            .iter_mut()
            .find(|slot| slot.is_none())
            .unwrap_or_else(|| unreachable!("a pool takes back only the permits it held"));
        *slot = Some(permit);
        drop(free);
        self.wake_leader();
    }
}

#[cfg(test)]
mod tests;
