//! The HIL observer's build identity.
//!
//! The runner is the observer of every HIL run, and the qualification
//! evaluator independently decides whether a run's observer still observes
//! the checkout. Both compute the same identity from the same parts, defined
//! here: the projection of the runner's resolved dependency graph onto the
//! inputs each workload uses ([`inputs`], [`cargo_inputs`]), Cargo's actual
//! compilation units bound to the executable ([`artifacts`]), the content
//! store a run's observer record refers to ([`store`]) and the receipt an
//! invocation of the runner carries ([`receipt`]). The `producer` feature
//! adds what runs Cargo: building the runner ([`compile`]), resolving its
//! graph ([`resolve`]) and preparing its receipt ([`prepare`]); evaluators
//! read prepared descriptors instead.
#![forbid(unsafe_code)]

pub mod artifacts;
pub mod cargo_inputs;
pub mod inputs;
pub mod receipt;
pub mod store;

#[cfg(feature = "producer")]
pub mod compile;
#[cfg(feature = "producer")]
pub mod prepare;
#[cfg(feature = "producer")]
pub mod resolve;
