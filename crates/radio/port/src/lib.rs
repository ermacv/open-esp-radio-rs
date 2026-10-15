#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! Shared radio capacity, lifecycle and operation contracts.
//!
//! [`run_port`] consumes an [`Installed`] backend and its independent driver.
//! Its [`Owner`] borrows the backend and consumes each typestate transition.
//! [`Mint::into_pools`] issues each installation's complete capacity once.
//! The driver owns only unbranded state: queues containing branded slots and
//! in-flight work belong in [`PortBackend::Held`], borrowed by the driver.
//! See [`lifecycle`] for retirement, poisoning and cancellation.

#[cfg(any(test, all(feature = "test-support", not(target_os = "none"))))]
extern crate std;

#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
pub mod lifecycle;
pub mod pool;
mod relation;
mod work;

pub use lifecycle::*;
pub use work::*;

pub use pool::{
    Child, Line, Mint, Permit, PermitPool, PermitSupply, PermitTokens, Permits, PoolBuilder,
    PoolPath, PoolSet, Pools, Root, Seat, Slot, Slots, branded,
};
pub use relation::{
    ClockInfo, ClockRelation, ClockSample, Generation, GenerationSource, Projected,
    ProjectionError, RadioStamp, RelationRate, RelationSample, Stamp,
};

/// The result of a port call: `Ok(Ok(_))` served, `Ok(Err(refusal))`
/// refused with nothing changed, `Err(Poisoned)` a poisoned backend.
pub type PortResult<T, R, C> = Result<Result<T, R>, Poisoned<C>>;

/// The backend dropped events it could not hold.
///
/// The marker takes the place of the first dropped event: every event
/// reported before it was produced before the gap, and every event reported
/// after it was produced after every dropped one. It is reported once per
/// gap, and never silently discarded while the port lives; a gap the port's
/// uninstall leaves ends with its stream.
///
/// A backend reserves the slot of every terminal event, and of data its
/// protocol promises to deliver, when it admits the work, so the gap holds
/// only data the protocol does not promise (received frames, advertising
/// reports), which is gone; the consumer continues after it.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct EventsLost;

/// A poisoned backend: its state is unknown and only a reset restores it.
///
/// It is the only outer error of a port call. A port's event consumer
/// returns it after every event produced before the fault and again at
/// every later call, so a consumer that awaits events learns of the fault;
/// every other call returns it at once. `cause` is the backend's own
/// account of the fault, which portable code passes on without
/// interpreting it.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct Poisoned<C> {
    /// Why the backend's state is unknown.
    pub cause: C,
}

#[cfg(test)]
mod tests;
