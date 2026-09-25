//! Shared HIL evidence contract.
//!
//! The runner produces evidence and the qualification evaluator independently
//! assesses it; both must compute the same observer build identity and the
//! same canonical scenario form. This crate owns those definitions. Evaluation
//! decisions stay with each consumer.
#![forbid(unsafe_code)]

pub mod artifacts;
pub mod cargo_inputs;
pub mod observer;
pub mod scenario;

// Producer operations run Cargo; evaluators read prepared descriptors instead.
#[cfg(feature = "producer")]
pub mod compile;
#[cfg(feature = "producer")]
pub mod resolve;
