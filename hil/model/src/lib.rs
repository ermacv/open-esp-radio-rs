//! The HIL scenario model ([#761]): what a scenario is, independently of
//! how a run executes it.
//!
//! A scenario is a Rust type implementing [`Spec`]: its stable
//! [`ScenarioId`], its one [`Purpose`], the contract it exercises
//! ([`Subject`]), typed parameters, criteria and trace, the
//! [`Resources`] and [`TargetCapabilities`] it needs, an [`Estimate`]
//! derived from its waits, and a pure [`Spec::verdict`] over a recorded
//! trace. Because the verdict is a pure function of the trace and the
//! criteria, the qualification evaluator recomputes it without a new run;
//! this crate is a `formats` package so that it may.
//!
//! Time is never a literal here. A [`Wait`] is a contract bound the image
//! reports, a protocol interval or a span measured on the stand
//! ([`StandProfile`]); [`repetitions`] derives how often to repeat from
//! the property being shown; [`TimeFit`] maps a source's clock onto the
//! run's single time axis.
//!
//! [#761]: https://github.com/ermacv/open-esp-radio-rs/issues/761
#![forbid(unsafe_code)]

mod capabilities;
mod estimate;
mod id;
mod identity;
mod profile;
mod purpose;
pub mod repetitions;
mod resources;
mod spec;
mod tier;
mod time;
mod verdict;
mod wait;

pub use capabilities::{CapabilityKey, CompositionConflict, CompositionName, TargetCapabilities};
pub use estimate::Estimate;
pub use id::{InstanceName, InvalidId, ScenarioId, validate_name};
pub use identity::{Identity, identity};
pub use profile::{MeasuredSpan, ProfileError, RunRef, SpanName, StandProfile};
pub use purpose::Purpose;
pub use resources::{
    AirFootprint, Airtime, Band, BoardCapability, BoardNeed, BoardOffer, BoardRole, LpClockSource,
    Resources, Tolerance, Unmet,
};
pub use spec::{Instance, Spec, Subject, criteria_from_json, params_from_json};
pub use tier::Tier;
pub use time::{Exchange, FitError, Mapped, TimeFit};
pub use verdict::{Agreement, Finding, Verdict};
pub use wait::{BoundName, Factor, InvalidFactor, MissingBound, ReportedBounds, Wait};
