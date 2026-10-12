//! The transmit permits a lower-MAC backend on the owner shape keeps.
//!
//! The lower-MAC port (`oer_ieee80211_lower_mac::owner`) states what a
//! client awaits only as a contract: a queue grant (`QueueGrant`) and a
//! transmit buffer, each the backend's own type. This module is the keeper
//! of those permits, which a backend holds in its held part of the install
//! (`PortBackend::Held`), and their guards, [`QueuePermit`] and
//! [`BufferPermit`]. It waits on its pools, so it lives here and not in the
//! sans-IO protocol. The guards and the keeper share one private module:
//! only [`TxPermits`] makes a guard, from the pool of the queue it names,
//! so no caller forges one, by type; a guard admitted into another keeper
//! of the install than the one that made it panics at admission, a fault of
//! a backend that keeps two.
//!
//! A client takes a queue and a buffer with its seats
//! ([`TxSeats`](oer_ieee80211_lower_mac::TxSeats)): [`TxPermits::queue`]
//! waits in the line of the queue its seat's access category occupies,
//! [`TxPermits::buffer`] in the buffers' line. Every queue's pool takes the
//! queue line ([`QueuePool::of`]) and holds a place for each of its seats;
//! `oer-radio-permit-pool`'s allocation-free `FifoPermitPool` is one.
//!
//! [`TxPermits::new`] mints one permit per queue the capabilities state and
//! builds each queue's pool from it ([`QueuePool::of`]), so a queue never
//! holds two attempts; a queue count other than the capabilities' does not
//! compile. A submission moves an attempt's two permits into the in-flight
//! table, indexed by its queue ([`TxPermits::admit`]), and the backend gives
//! them back when it takes the attempt's terminal, before it hands the event
//! out ([`TxPermits::complete`]): no caller holds a submitted attempt's
//! permits, and a terminal event dropped unread loses none.
//!
//! The `test-support` feature adds `model`, an in-memory lower-MAC backend
//! on the owner shape built over these permits.

mod permits;

pub use permits::{BufferPermit, QueuePermit, QueuePool, TxBufferKind, TxPermits, TxQueueKind};

#[cfg(any(test, feature = "test-support"))]
pub mod model;
