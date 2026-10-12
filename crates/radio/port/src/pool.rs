//! Capacity as values: the slot pools and permit pools of one install.
//!
//! A port's capacity is never a refusal ([ownership and refusals]). What
//! the hardware holds at once (key slots, Block Ack slots, accept-list
//! entries, a transmit queue, the one operation of a radio) is a set of
//! tokens the install mints once, through the [`Mint`] that
//! [`run_port`](crate::run_port) hands its backend, branded by the install
//! (`'id`) and by the pool's kind marker `Kind`, which keeps pools apart
//! (a Wi-Fi station's key slots and an access point's; a Bluetooth LE
//! advertising set and a connection).
//!
//! - A **slot** ([`Slot`]) is taken synchronously: a pool is an array of
//!   slots its holder owns, so how many are left is in the types, and an
//!   empty pool is an array of none, not a run-time `None`. Installing a
//!   resource moves a slot into it, and removing the resource gives the
//!   slot back.
//! - A **permit** ([`Permit`]) is awaited: a [`PermitPool`] holds a fixed
//!   number, and taking one waits while all are held, never refused. A
//!   submission moves its permit into the backend, which keeps the pool in
//!   its held part of the install (`PortBackend::Held`) and takes the
//!   permit back when the work ends, before it hands the terminal event
//!   out: released when the air finished, not when a handle drops, and
//!   never lost with an event dropped unread; a cancel is only a request. In which order waiting
//!   takers are served is the pool's documented policy (first in, first out
//!   by default), which the conformance suite checks.
//!
//! A caller counts its own slots: it plans against the same constant the
//! backend minted its pool with, so taking a slot synchronously is an
//! invariant of that count (popping the array it holds), never a refusal.
//!
//! The clients of one install share a pool only inside the body's task. A
//! token is branded, so it never enters `'static` storage such as an
//! interrupt handler's: a backend keeps the tokens on the task side, keyed
//! by their [`index`](Permit::index), and its interrupt carries only the
//! index.
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
//! fn forge<'id, Kind>() -> Slot<'id, Kind> {
//!     Slot { index: 0, brand: PhantomData, kind: PhantomData }
//! }
//! ```
//!
//! ```compile_fail,E0382
//! # use oer_radio_port::Slot;
//! fn install_twice<'id, Kind>(slot: Slot<'id, Kind>, install: impl Fn(Slot<'id, Kind>)) {
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
//! fn mix<'a, 'b, Kind>(slot: Slot<'a, Kind>) -> Slot<'b, Kind> {
//!     slot
//! }
//! ```
//!
//! ```compile_fail
//! # use oer_radio_port::Slot;
//! fn keep<'id, Kind>(slot: Slot<'id, Kind>) -> Slot<'static, Kind> {
//!     slot
//! }
//! ```
//!
//! The brand is invariant: even a brand that outlives another does not
//! shrink to it, so a slot of an outer install never passes for an inner
//! one's, nor an owner for another's:
//!
//! ```compile_fail
//! # use oer_radio_port::Slot;
//! fn shrink<'a: 'b, 'b, K>(slot: Slot<'a, K>) -> Slot<'b, K> {
//!     slot
//! }
//! ```
//!
//! ```compile_fail
//! # use oer_radio_port::{Disabled, Owner, PortBackend};
//! fn shrink<'a: 'b, 'b, B: PortBackend>(owner: Owner<'a, B, Disabled>) -> Owner<'b, B, Disabled> {
//!     owner
//! }
//! ```
//!
//! Their compiling twins move a slot within its own install:
//!
//! ```
//! # use oer_radio_port::Slot;
//! fn install_once<'id, Kind>(slot: Slot<'id, Kind>, install: impl Fn(Slot<'id, Kind>)) {
//!     install(slot);
//! }
//! fn same<'a, Kind>(slot: Slot<'a, Kind>) -> Slot<'a, Kind> {
//!     slot
//! }
//! fn index<Kind>(slot: &Slot<'_, Kind>) -> u8 {
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

    /// The `N` slots of a pool of the kind `Kind`, indexed 0 to `N - 1`.
    /// `N` is a plain constant: a backend generic over several pool sizes
    /// mints one pool per hardware pool rather than summing them.
    pub fn slots<Kind, const N: usize>(&mut self) -> [Slot<'id, Kind>; N] {
        const { assert!(N <= 256, "a pool holds at most 256 tokens") };
        core::array::from_fn(|index| Slot {
            index: index_of(index),
            brand: PhantomData,
            kind: PhantomData,
        })
    }

    /// The `N` permits of a pool of the kind `Kind`, indexed 0 to `N - 1`,
    /// which the backend's [`PermitPool`] hands out and takes back.
    pub fn permits<Kind, const N: usize>(&mut self) -> [Permit<'id, Kind>; N] {
        const { assert!(N <= 256, "a pool holds at most 256 tokens") };
        core::array::from_fn(|index| Permit {
            index: index_of(index),
            brand: PhantomData,
            kind: PhantomData,
        })
    }
}

/// The index of a token of a pool of at most 256, which `Mint` asserts at
/// compile time.
#[allow(
    clippy::cast_possible_truncation,
    reason = "a pool holds at most 256 tokens"
)]
const fn index_of(index: usize) -> u8 {
    index as u8
}

/// One slot of an install's pool of the kind `Kind`: the capacity of one
/// installed resource. Installing a resource moves it in; removing the
/// resource gives it back.
#[must_use = "a slot is capacity; dropping it loses it until the next install"]
pub struct Slot<'id, Kind> {
    index: u8,
    brand: Brand<'id>,
    kind: PhantomData<fn() -> Kind>,
}

impl<Kind> Slot<'_, Kind> {
    /// The slot's index in its pool, for the backend that places the
    /// resource.
    pub const fn index(&self) -> u8 {
        self.index
    }
}

/// One permit of a pool of the kind `Kind`: one unit of capacity a piece of
/// work holds until it ends; the backend that keeps the pool gives it back
/// before it hands the work's terminal out.
#[must_use = "a permit goes back to its pool through `PermitPool::release`"]
pub struct Permit<'id, Kind> {
    index: u8,
    brand: Brand<'id>,
    kind: PhantomData<fn() -> Kind>,
}

impl<Kind> Permit<'_, Kind> {
    /// The permit's index in its pool, for the backend that places the
    /// work.
    pub const fn index(&self) -> u8 {
        self.index
    }
}

/// A pool of the permits of one install of the kind `Kind`, which a runtime
/// crate or a backend keeps; it lives in one task, shared by reference by
/// the port's body and its phases, never across executor tasks.
pub trait PermitPool<'id, Kind> {
    /// Take a permit, waiting while every permit is held, never refused, in
    /// the order the pool documents (first in, first out by default).
    /// Dropping the future gives up its place.
    fn take(&self) -> impl Future<Output = Permit<'id, Kind>> + '_;

    /// Give a permit back: the backend that keeps the pool calls it when the
    /// work that held the permit ended, before it hands the terminal out,
    /// or when a taker drops a permit it never submitted; the next waiting
    /// taker resumes.
    fn release(&self, permit: Permit<'id, Kind>);
}
