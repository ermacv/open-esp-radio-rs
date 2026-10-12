//! Capacity as values: the slot pools and permit pools of one install.
//!
//! A port's capacity is never a refusal ([ownership and refusals]). What
//! the hardware holds at once (key slots, Block Ack slots, accept-list
//! entries, a transmit queue, the one operation of a radio) is a set of
//! tokens the install mints once, through the [`Mint`] that
//! [`run_port`](crate::run_port) hands its backend, branded by the install
//! (`'id`) and by the capabilities marker `K` they count for.
//!
//! - A **slot** ([`Slot`]) is taken synchronously: a pool is an array of
//!   slots its holder owns, so how many are left is in the types, and an
//!   empty pool is an array of none, not a run-time `None`. Installing a
//!   resource moves a slot into it, and removing the resource gives the
//!   slot back.
//! - A **permit** ([`Permit`]) is awaited: a [`PermitPool`] holds a fixed
//!   number, and taking one waits while all are held, never refused. A
//!   submission moves its permit into the work, and the work's terminal
//!   event carries it back, released when the air finished, not when a
//!   handle drops; a cancel is only a request. In which order waiting
//!   takers are served is the pool's documented policy (first in, first out
//!   by default), which the conformance suite checks.
//!
//! This package owns no state and never waits: it states the tokens and the
//! rules. A pool's free list, its waiters and their wake-ups belong to a
//! runtime crate or to the backend that keeps it. The backend never refuses
//! for capacity: a caller takes synchronously from the slots it holds, and
//! one that holds none turns that into its protocol's decision (an access
//! point refuses an association).
//!
//! Neither is `Clone`, `Copy` or `Default`, and only a [`Mint`] makes one,
//! so no caller adds capacity:
//!
//! ```compile_fail,E0599
//! # use oer_radio_port::Slot;
//! fn copy<'id, K>(slot: &Slot<'id, K>) -> Slot<'id, K> {
//!     slot.clone()
//! }
//! ```
//!
//! ```compile_fail,E0451
//! # use core::marker::PhantomData;
//! # use oer_radio_port::Slot;
//! fn forge<'id, K>() -> Slot<'id, K> {
//!     Slot { index: 0, brand: PhantomData, capabilities: PhantomData }
//! }
//! ```
//!
//! ```compile_fail,E0382
//! # use oer_radio_port::Slot;
//! fn install_twice<'id, K>(slot: Slot<'id, K>, install: impl Fn(Slot<'id, K>)) {
//!     install(slot);
//!     install(slot);
//! }
//! ```
//!
//! A slot of one install does not reach another, nor leaves its body for
//! `'static`; neither has an error code of its own (a lifetime error):
//!
//! ```compile_fail
//! # use oer_radio_port::Slot;
//! fn mix<'a, 'b, K>(slot: Slot<'a, K>) -> Slot<'b, K> {
//!     slot
//! }
//! ```
//!
//! ```compile_fail
//! # use oer_radio_port::Slot;
//! fn keep<'id, K>(slot: Slot<'id, K>) -> Slot<'static, K> {
//!     slot
//! }
//! ```
//!
//! Their compiling twins move a slot within its own install:
//!
//! ```
//! # use oer_radio_port::Slot;
//! fn install_once<'id, K>(slot: Slot<'id, K>, install: impl Fn(Slot<'id, K>)) {
//!     install(slot);
//! }
//! fn same<'a, K>(slot: Slot<'a, K>) -> Slot<'a, K> {
//!     slot
//! }
//! fn index<K>(slot: &Slot<'_, K>) -> u8 {
//!     slot.index()
//! }
//! ```
//!
//! [ownership and refusals]: https://github.com/ermacv/open-esp-radio-rs/blob/main/docs/architecture.md#ownership-and-refusals

use core::{future::Future, marker::PhantomData};

/// The brand of one install.
type Brand<'id> = PhantomData<fn(&'id ()) -> &'id ()>;

/// The one maker of an install's slots and permits. [`run_port`](crate::run_port)
/// hands it to the backend's `resources` once and no caller ever holds
/// one, so every slot and permit of an install counts real capacity. It
/// keeps no state: the backend places what it mints.
pub struct Mint<'id> {
    brand: Brand<'id>,
}

impl<'id> Mint<'id> {
    pub(crate) const fn new() -> Self {
        Self { brand: PhantomData }
    }

    /// The `N` slots of a pool counting for `K`, indexed 0 to `N - 1`.
    pub fn slots<K, const N: usize>(&mut self) -> [Slot<'id, K>; N] {
        core::array::from_fn(|index| Slot {
            index: index_of(index),
            brand: PhantomData,
            capabilities: PhantomData,
        })
    }

    /// The `N` permits of a pool counting for `K`, indexed 0 to `N - 1`,
    /// which the backend's [`PermitPool`] hands out and takes back.
    pub fn permits<K, const N: usize>(&mut self) -> [Permit<'id, K>; N] {
        core::array::from_fn(|index| Permit {
            index: index_of(index),
            brand: PhantomData,
            capabilities: PhantomData,
        })
    }
}

fn index_of(index: usize) -> u8 {
    u8::try_from(index).expect("a pool holds at most 256 tokens")
}

/// One slot of an install's pool counting for `K`: the capacity of one
/// installed resource. Installing a resource moves it in; removing the
/// resource gives it back.
#[must_use = "a slot is capacity; dropping it loses it until the next install"]
pub struct Slot<'id, K> {
    index: u8,
    brand: Brand<'id>,
    capabilities: PhantomData<fn() -> K>,
}

impl<K> Slot<'_, K> {
    /// The slot's index in its pool, for the backend that places the
    /// resource.
    pub const fn index(&self) -> u8 {
        self.index
    }
}

/// One permit of a pool counting for `K`: one unit of capacity a piece of
/// work holds until its terminal event carries it back.
#[must_use = "a permit goes back to its pool through `PermitPool::release`"]
pub struct Permit<'id, K> {
    index: u8,
    brand: Brand<'id>,
    capabilities: PhantomData<fn() -> K>,
}

impl<K> Permit<'_, K> {
    /// The permit's index in its pool, for the backend that places the
    /// work.
    pub const fn index(&self) -> u8 {
        self.index
    }
}

/// A pool of the permits of one install counting for `K`, which a runtime
/// crate or a backend keeps; it lives in one task, shared by reference by
/// the port's body and its phases, never across executor tasks.
pub trait PermitPool<'id, K> {
    /// Take a permit, waiting while every permit is held, never refused, in
    /// the order the pool documents (first in, first out by default).
    /// Dropping the future gives up its place.
    fn take(&self) -> impl Future<Output = Permit<'id, K>> + '_;

    /// Give a permit back, when the terminal event of the work that held it
    /// arrived; the next waiting taker resumes.
    fn release(&self, permit: Permit<'id, K>);
}
