//! The contract between the HIL producers and the qualification evaluator.
//!
//! The runner produces evidence and the qualification evaluator independently
//! assesses it; both must read the same run vocabulary, the same canonical
//! scenario form and the same source-snapshot manifest. This crate owns those
//! definitions. The observer's build identity is
//! [`oer-hil-observer`](../observer/README.md)'s, and the run bundle's format
//! [`oer-hil-run-bundle`](../host/run-bundle/README.md)'s. Evaluation
//! decisions stay with each consumer.
#![forbid(unsafe_code)]

pub mod dependency;
pub mod image;
pub mod run;
pub mod scenario;
pub mod snapshot;
