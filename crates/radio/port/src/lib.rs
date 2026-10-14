#![no_std]
#![forbid(unsafe_code)]
#![deny(missing_docs)]

//! The vocabulary every radio port shares.
//!
//! A radio port (`docs/architecture.md`, "Radio ports") is the contract
//! between the portable logic of one radio protocol and the backend that
//! executes it. Each radio block owns its port's lifecycle (its owner, body,
//! backend and joined driver: `oer-ieee80211-port`, `oer-ieee802154-port`,
//! `oer-bluetooth-port`); this package declares only what they share:
//!
//! - [`pool`]: capacity as branded tokens: [`branded`] runs an install with
//!   the [`Mint`] of a fresh brand, which makes its [`Slot`]s, taken
//!   synchronously, [`Permit`]s, awaited from a [`PermitPool`] in its
//!   documented order, and [`Seat`]s, the places in a pool's [`Line`] its
//!   takers wait with.
//! - [`Poisoned`] and [`PortResult`]: the only outer error of a port call,
//!   carrying the backend's cause; every refusal is the inner `Err`.
//! - [`EventsLost`]: the one loss marker, with its ordering rule.
//! - [`ClockInfo`]: the resolution of a port's radio clock and its
//!   [`ClockRelation`] to the image's monotonic time, if one is known.
//! - [`ClockRelation`], [`RelationSample`], [`Generation`] and [`Stamp`]:
//!   the one relation between two clock domains and its projection.
//!
//! This package owns no state and never waits.

#[cfg(any(test, all(feature = "test-support", not(target_os = "none"))))]
extern crate std;

#[cfg(any(test, feature = "test-support"))]
pub mod conformance;
pub mod pool;
mod relation;

pub use pool::{
    Line, Mint, Permit, PermitPool, PermitTokens, Permits, PoolBuilder, PoolSet, Pools, Seat, Slot,
    Slots, branded,
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
