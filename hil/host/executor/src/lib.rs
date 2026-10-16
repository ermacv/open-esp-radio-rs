//! Execution of typed HIL scenarios ([#761]).
//!
//! A scenario is an [`oer_hil_model::Spec`]; executing it is [`Run<X>`]
//! for an [`Executor`] `X`: [`Model`] (the host model) or [`Stand`]. A
//! conformance scenario is generic over its port's harness trait, so the
//! model's harness and the stand's proxy (which asks the target to exercise
//! a case and decodes the trace it reports) run the same scenario code, and
//! the same pure verdict judges both.
//!
//! [`execute`] admits a scenario only on an executor whose kind runs its
//! purpose ([`RunsOn`]): the model runs conformance only. An interop
//! scenario on the model does not compile:
//!
//! ```compile_fail,E0277
//! # use oer_hil_executor::{Infrastructure, Model, Recorder, Run, execute};
//! # use oer_hil_model::*;
//! # struct Peer;
//! # impl Subject for Peer {
//! #     const CONTRACT: &'static str = "toy::Peer";
//! #     fn declared_bounds() -> ReportedBounds { ReportedBounds::default() }
//! # }
//! struct WithHostapd;
//! impl Spec for WithHostapd {
//!     const ID: ScenarioId = name!(ScenarioId, "with-hostapd");
//!     type Purpose = Interop;
//! #   type Subject = Peer;
//! #   type Params = ();
//! #   type Criteria = ();
//! #   type Trace = ();
//! #   fn resources((): &()) -> Resources { Resources::default() }
//! #   fn target((): &()) -> TargetCapabilities {
//! #       TargetCapabilities { composition: Composition::Any, capabilities: Default::default() }
//! #   }
//! #   fn estimate((): &(), _: &Bounds, _: &StandProfile) -> Result<Estimate, WaitError> {
//! #       Ok(Estimate::default())
//! #   }
//! #   fn verdict((): &(), (): &()) -> Verdict { Verdict::Pass }
//! }
//! impl<X: oer_hil_executor::Executor> Run<X> for WithHostapd {
//!     async fn run((): &(), _: &mut X, _: &mut Recorder) -> Result<(), Infrastructure> { Ok(()) }
//! }
//! let instance = Instance::<WithHostapd>::new(name!(InstanceName, "open"), ());
//! let _ = execute(&instance, &(), &mut Model(()));
//! ```
//!
//! The same scenario on the stand compiles:
//!
//! ```
//! # use oer_hil_executor::{Infrastructure, Recorder, Run, Stand, execute};
//! # use oer_hil_model::*;
//! # struct Peer;
//! # impl Subject for Peer {
//! #     const CONTRACT: &'static str = "toy::Peer";
//! #     fn declared_bounds() -> ReportedBounds { ReportedBounds::default() }
//! # }
//! struct WithHostapd;
//! impl Spec for WithHostapd {
//!     const ID: ScenarioId = name!(ScenarioId, "with-hostapd");
//!     type Purpose = Interop;
//! #   type Subject = Peer;
//! #   type Params = ();
//! #   type Criteria = ();
//! #   type Trace = ();
//! #   fn resources((): &()) -> Resources { Resources::default() }
//! #   fn target((): &()) -> TargetCapabilities {
//! #       TargetCapabilities { composition: Composition::Any, capabilities: Default::default() }
//! #   }
//! #   fn estimate((): &(), _: &Bounds, _: &StandProfile) -> Result<Estimate, WaitError> {
//! #       Ok(Estimate::default())
//! #   }
//! #   fn verdict((): &(), (): &()) -> Verdict { Verdict::Pass }
//! }
//! impl<X: oer_hil_executor::Executor> Run<X> for WithHostapd {
//!     async fn run((): &(), _: &mut X, _: &mut Recorder) -> Result<(), Infrastructure> { Ok(()) }
//! }
//! let instance = Instance::<WithHostapd>::new(name!(InstanceName, "open"), ());
//! let _ = execute(&instance, &(), &mut Stand(()));
//! ```
//!
//! The [`Recorder`] keeps what a run counted: protocol retries
//! ([`ProtocolRetry`]), infrastructure retries and measurements, a name
//! being one or the other. [`plan`] refuses what the stand lacks, checks a
//! tier against its budget before anything is built or leased, groups it
//! into image segments, derives each segment's lease limit and labels every
//! provisional value it used.
//!
//! Its `clippy.toml`, a copy of the HIL scenario crates' group, forbids
//! duration literals and `thread::sleep`: waits come from
//! [`oer_hil_model::Wait`].
//!
//! [#761]: https://github.com/ermacv/open-esp-radio-rs/issues/761
#![forbid(unsafe_code)]

mod execute;
mod executor;
pub mod plan;
mod recorder;
mod retry;

pub use execute::{Cause, Failed, Infrastructure, Outcome, Run, execute};
pub use executor::{Executed, Executor, ExecutorKind, Model, OnModel, OnStand, RunsOn, Stand};
pub use recorder::{INFRASTRUCTURE_RETRIES, InfrastructureRetry, RecordError, Recorder};
pub use retry::{AdmitsLoss, Attempt, ProtocolRetry, RetryEnd};
