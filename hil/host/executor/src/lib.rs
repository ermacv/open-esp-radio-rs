//! Execution of typed HIL scenarios ([#761]).
//!
//! A scenario is an [`oer_hil_model::Spec`]; executing it is [`Run<X>`]
//! for an [`Executor`] `X`: [`Model`] around a [`ModelDriver`], or
//! [`Stand`] around a [`StandDriver`], which holds the [`StandSession`] of
//! a granted stand lease. A conformance scenario is generic over its
//! port's harness trait, so the model's harness and the stand's proxy
//! (which asks the target to exercise a case and decodes the trace it
//! reports) run the same scenario code, and the same pure verdict judges
//! both.
//!
//! [`execute`] admits a scenario only on an executor whose kind runs its
//! purpose ([`RunsOn`]): the model runs conformance only. An interop
//! scenario on the model does not compile:
//!
//! ```compile_fail,E0277
//! # use oer_hil_executor::*;
//! # use oer_hil_model::*;
//! # struct Peer;
//! # impl Subject for Peer {
//! #     const CONTRACT: ContractName = name!(ContractName, "toy::Peer");
//! #     fn ceilings() -> ReportedBounds { ReportedBounds::default() }
//! # }
//! struct WithHostapd;
//! impl Spec for WithHostapd {
//!     const ID: ScenarioId = name!(ScenarioId, "with-hostapd");
//!     type Purpose = Interop;
//! #   type Subject = Peer;
//! #   type Params = ();
//! #   type Criteria = ();
//! #   type Trace = ();
//! #   type Finding = ();
//! #   fn resources((): &()) -> Resources { Resources::default() }
//! #   fn target((): &()) -> TargetCapabilities {
//! #       TargetCapabilities { chip: name!(ChipId, "esp32s31"), composition: Composition::Any, capabilities: Default::default() }
//! #   }
//! #   fn estimate((): &(), c: &ReportedBounds, p: &StandProfile) -> Result<Estimate, WaitError> {
//! #       Estimate::from_waits(&[], &[], 0, c, p)
//! #   }
//! #   fn verdict((): &(), (): &()) -> Verdict<()> { Verdict::Pass }
//! }
//! impl<X: Executor> Run<X> for WithHostapd {
//!     async fn run((): &(), _: &mut X, _: &mut Recorder) -> Result<(), Infrastructure> { Ok(()) }
//! }
//! struct ModelPeer;
//! impl ModelDriver for ModelPeer {}
//! let instance = Instance::<WithHostapd>::new(name!(InstanceName, "open"), ());
//! let _ = execute(&instance, &(), &mut Model::new(ModelPeer));
//! ```
//!
//! The same scenario on the stand compiles:
//!
//! ```
//! # use oer_hil_executor::*;
//! # use oer_hil_model::*;
//! # struct Peer;
//! # impl Subject for Peer {
//! #     const CONTRACT: ContractName = name!(ContractName, "toy::Peer");
//! #     fn ceilings() -> ReportedBounds { ReportedBounds::default() }
//! # }
//! struct WithHostapd;
//! impl Spec for WithHostapd {
//!     const ID: ScenarioId = name!(ScenarioId, "with-hostapd");
//!     type Purpose = Interop;
//! #   type Subject = Peer;
//! #   type Params = ();
//! #   type Criteria = ();
//! #   type Trace = ();
//! #   type Finding = ();
//! #   fn resources((): &()) -> Resources { Resources::default() }
//! #   fn target((): &()) -> TargetCapabilities {
//! #       TargetCapabilities { chip: name!(ChipId, "esp32s31"), composition: Composition::Any, capabilities: Default::default() }
//! #   }
//! #   fn estimate((): &(), c: &ReportedBounds, p: &StandProfile) -> Result<Estimate, WaitError> {
//! #       Estimate::from_waits(&[], &[], 0, c, p)
//! #   }
//! #   fn verdict((): &(), (): &()) -> Verdict<()> { Verdict::Pass }
//! }
//! impl<X: Executor> Run<X> for WithHostapd {
//!     async fn run((): &(), _: &mut X, _: &mut Recorder) -> Result<(), Infrastructure> { Ok(()) }
//! }
//! struct Hostapd(StandSession<'static>);
//! impl StandDriver<'static> for Hostapd {
//!     fn session(&self) -> &StandSession<'static> { &self.0 }
//! }
//! let instance = Instance::<WithHostapd>::new(name!(InstanceName, "open"), ());
//! let _ = execute(&instance, &(), &mut Stand::new(Hostapd(StandSession::for_tests())));
//! ```
//!
//! A model driver cannot pose as the stand, because a stand driver holds
//! the session of a granted lease:
//!
//! ```compile_fail,E0277
//! # use oer_hil_executor::*;
//! struct ModelPort;
//! impl ModelDriver for ModelPort {}
//! let _ = Stand::new(ModelPort);
//! ```
//!
//! Nor can any other executor claim the stand's kind, since only
//! [`Model`] and [`Stand`] are executors:
//!
//! ```compile_fail,E0277
//! # use oer_hil_executor::*;
//! struct ModelPort;
//! impl ModelDriver for ModelPort {}
//! struct Fake(ModelPort);
//! impl Executor for Fake {
//!     type Kind = OnStand;
//!     type Driver = ModelPort;
//!     fn driver(&mut self) -> &mut ModelPort { &mut self.0 }
//! }
//! ```
//!
//! A driver made under a lease can:
//!
//! ```
//! # use oer_hil_executor::*;
//! struct Proxy(StandSession<'static>);
//! impl StandDriver<'static> for Proxy {
//!     fn session(&self) -> &StandSession<'static> { &self.0 }
//! }
//! let _ = Stand::new(Proxy(StandSession::for_tests()));
//! ```
//!
//! The [`Recorder`] keeps what a run measured, a name being a count or a
//! measurement, never both. [`plan`] refuses what the stand lacks, checks a
//! tier against its budget before anything is built or leased, groups it
//! into image segments, adds each instance's boot, derives each segment's
//! lease limit and labels every provisional stand span it used.
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

pub use execute::{Cause, Failed, Infrastructure, Outcome, Run, execute};
pub use executor::{
    Executed, Executor, ExecutorKind, Model, ModelDriver, OnModel, OnStand, RunsOn, Stand,
    StandDriver, StandSession,
};
pub use recorder::{RecordError, Recorder};
