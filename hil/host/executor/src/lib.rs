//! Execution of typed HIL scenarios ([#761]).
//!
//! A scenario is an [`oer_hil_model::Spec`]; executing it is [`Run<X>`],
//! implemented for every executor `X` the scenario can use. A conformance
//! scenario is generic over its port's harness trait, so the host model's
//! harness and the stand's proxy (which asks the target to exercise a case
//! and returns the trace it reports) run the same scenario code, and the
//! same pure verdict judges both.
//!
//! [`execute`] runs one instance: the trace comes from the run, the verdict
//! from [`Spec::verdict`](oer_hil_model::Spec::verdict), and the
//! [`Recorder`] keeps what the run counted: protocol retries
//! ([`ProtocolRetry`]), infrastructure retries and measurements.
//! [`plan`] checks a tier against its budget before anything is built or
//! leased, groups it into image segments and derives each segment's lease
//! limit.
//!
//! Its `clippy.toml` forbids duration literals and `thread::sleep`: waits
//! come from [`oer_hil_model::Wait`].
//!
//! [#761]: https://github.com/ermacv/open-esp-radio-rs/issues/761
#![forbid(unsafe_code)]

mod execute;
pub mod plan;
mod recorder;
mod retry;

pub use execute::{Failed, Infrastructure, Outcome, Run, execute};
pub use recorder::{InfrastructureRetry, Recorder};
pub use retry::{AdmitsLoss, Attempt, ProtocolRetry, RetryEnd};
