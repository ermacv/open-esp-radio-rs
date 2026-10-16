//! The HIL scenario model ([#761]): what a scenario is, independently of
//! how a run executes it.
//!
//! A scenario is a Rust type implementing [`Spec`]: its stable
//! [`ScenarioId`], its one purpose as a marker type ([`Conformance`],
//! [`Interop`], [`Measurement`], [`Investigation`]), the contract it
//! exercises ([`Subject`], with the contract's bound ceilings), typed
//! parameters, criteria, trace and findings, the [`Resources`] and
//! [`TargetCapabilities`] it needs, an [`Estimate`] derived from its
//! waits, and a pure [`Spec::verdict`] over a recorded trace. Because the
//! verdict is a pure function of the trace and the criteria, the
//! qualification evaluator recomputes it without a new run; this crate is
//! a `formats` package so that it may.
//!
//! A conformance trace starts with a [`TraceHeader`]: the bounds the image
//! reports, checked against the contract's ceilings, and the
//! [`ClockDomain`] of its times.
//!
//! Time is never a literal here. A [`Wait`] is a contract ceiling, a typed
//! protocol interval ([`ProtocolQuantity`]) or a span measured on the
//! stand ([`StandProfile`]); arithmetic on it is checked ([`Overflow`]),
//! and a stand span used before its evidence suffices carries a
//! [`Provisional`] label.
//!
//! [#761]: https://github.com/ermacv/open-esp-radio-rs/issues/761
#![forbid(unsafe_code)]

mod arithmetic;
mod capabilities;
mod digest;
mod estimate;
mod finite;
mod measurement;
mod name;
mod profile;
mod purpose;
pub mod repetitions;
mod resources;
mod spec;
mod tier;
mod trace;
mod verdict;
mod wait;

pub use arithmetic::{Overflow, sum};
pub use capabilities::{Composition, ImageConflict, TargetCapabilities};
pub use digest::{Sha256, canonical_json};
pub use estimate::Estimate;
pub use finite::NonFinite;
pub use measurement::{MeasuredValue, Unit};
pub use name::{
    BoundName, CapabilityKey, ChipId, ClockName, CompositionName, ContractName, FixtureName,
    Grammar, InstanceName, InvalidName, MeasurementName, Operation, ScenarioId, ScenarioInstance,
    SpanName,
};
pub use profile::{
    CONFIDENCE, MeasuredSpan, ProfileError, ProfileErrors, QUANTILE, RawStandProfile, RunId,
    RunRef, StandProfile, Support,
};
pub use purpose::{Conformance, Interop, Investigation, Measurement, OpenQuestion, PurposeKind};
pub use resources::{
    BoardCapability, BoardNeed, BoardOffer, BoardRole, LpClockSource, Resources, StandOffer, Unmet,
};
pub use spec::{IdentityError, Instance, Spec, Subject};
pub use tier::Tier;
pub use trace::{ClockDomain, HeaderFinding, TraceHeader};
pub use verdict::{Findings, NoFinding, Verdict};
pub use wait::{
    ConnectionInterval, Factor, InvalidFactor, ProtocolQuantity, Provisional, ReportedBounds,
    Resolved, Symbols, TimeUnits, Wait, WaitError,
};
