//! The HIL scenario model ([#761]): what a scenario is, independently of
//! how a run executes it.
//!
//! A scenario is a Rust type implementing [`Spec`]: its stable
//! [`ScenarioId`], its one purpose as a marker type ([`Conformance`],
//! [`Interop`], [`Measurement`], [`Investigation`]), the contract it
//! exercises ([`Subject`]), typed parameters, criteria and trace, the
//! [`Resources`] and [`TargetCapabilities`] it needs, an [`Estimate`]
//! derived from its waits, and a pure [`Spec::verdict`] over a recorded
//! trace. Because the verdict is a pure function of the trace and the
//! criteria, the qualification evaluator recomputes it without a new run;
//! this crate is a `formats` package so that it may.
//!
//! Time is never a literal here. A [`Wait`] is a contract bound, a typed
//! protocol interval ([`ProtocolQuantity`]) or a span measured on the
//! stand ([`StandProfile`]); arithmetic on it is checked ([`Overflow`]),
//! and a value used before its evidence suffices carries a
//! [`Provisional`] label.
//!
//! [#761]: https://github.com/ermacv/open-esp-radio-rs/issues/761
#![forbid(unsafe_code)]

mod arithmetic;
mod capabilities;
mod digest;
mod estimate;
mod finite;
mod name;
mod profile;
mod purpose;
pub mod repetitions;
mod resources;
mod spec;
mod tier;
mod verdict;
mod wait;

pub use arithmetic::{Overflow, sum};
pub use capabilities::{Composition, CompositionConflict, TargetCapabilities};
pub use digest::{Sha256, canonical_json};
pub use estimate::Estimate;
pub use finite::NonFinite;
pub use name::{
    BoundName, CapabilityKey, ChipId, CompositionName, CountName, FixtureName, Grammar,
    InstanceName, InvalidName, Operation, ScenarioId, ScenarioInstance, SpanName,
};
pub use profile::{
    CONFIDENCE, MeasuredSpan, ProfileError, ProfileErrors, QUANTILE, RawStandProfile,
    RecordedBounds, RunId, RunRef, StandProfile, Support,
};
pub use purpose::{
    Conformance, Interop, Investigation, Measurement, OpenQuestion, Purpose, PurposeKind,
};
pub use resources::{
    BoardCapability, BoardNeed, BoardOffer, BoardRole, LpClockSource, Resources, StandOffer, Unmet,
};
pub use spec::{IdentityError, Instance, Spec, Subject};
pub use tier::Tier;
pub use verdict::{Finding, Verdict};
pub use wait::{
    Bounds, BoundsSource, Factor, InvalidFactor, ProtocolQuantity, Provisional, ReportedBounds,
    Resolved, Wait, WaitError,
};
