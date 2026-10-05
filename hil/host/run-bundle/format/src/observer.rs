//! The HIL observer's build identity, as data a run records.
//!
//! The runner is the observer of every HIL run, and the qualification
//! evaluator independently decides whether a run's observer still observes
//! the checkout. Both compute the same identity from the same parts, defined
//! here: the projection of the runner's resolved dependency graph onto the
//! inputs each workload uses ([`inputs`], [`cargo_inputs`]), Cargo's actual
//! compilation units bound to the executable ([`artifacts`]), the content
//! store a run's observer record refers to ([`store`]) and the receipt an
//! invocation of the runner carries ([`receipt`]). Building the runner,
//! resolving its graph and preparing its receipt is `oer-hil-observer`'s;
//! evaluators read the prepared descriptors here.

pub mod artifacts;
pub mod cargo_inputs;
pub mod inputs;
pub mod receipt;
pub mod store;
