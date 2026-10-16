//! A scenario's typed specification.

use core::fmt::Debug;

use serde::{Serialize, de::DeserializeOwned};

use crate::{
    Estimate, InstanceName, MissingBound, Purpose, ReportedBounds, Resources, ScenarioId,
    StandProfile, TargetCapabilities, Verdict,
};

/// The contract a scenario exercises, named by its path
/// (`crate::Trait`). A qualification proof is accepted only when its
/// scenario's subject is the capability's contract, so an incidental
/// scenario cannot stand in for the minimal one.
pub trait Subject {
    const CONTRACT: &'static str;
}

/// Everything about a scenario but its execution: identity, purpose,
/// typed parameters, criteria and trace, what it needs and how long it
/// takes, and its verdict.
///
/// Execution is a separate trait in the executor, so this one stays a
/// format the qualification evaluator links to recompute verdicts.
/// Parameters and criteria must serialize deterministically (structs,
/// sequences and ordered maps, no `HashMap`): their JSON is part of the
/// scenario's [`Identity`](crate::Identity).
pub trait Spec {
    const ID: ScenarioId;
    const PURPOSE: Purpose;
    type Subject: Subject;
    /// One instance's parameters; instances replace copied scenarios.
    type Params: Clone + Debug + Serialize + DeserializeOwned;
    /// Acceptance thresholds. A qualification requirement owns them; an
    /// investigation keeps them beside its instance.
    type Criteria: Clone + Debug + Serialize + DeserializeOwned;
    /// What a run records; the verdict's only input besides the criteria.
    type Trace: Debug + Serialize + DeserializeOwned;

    /// Boards by role, fixtures, the air footprint and board capabilities.
    fn resources(params: &Self::Params) -> Resources;

    /// The target capabilities the image must serve; the runner builds one
    /// image for the union over a tier.
    fn target(params: &Self::Params) -> TargetCapabilities;

    /// The time one instance takes, from its waits and its repetitions.
    fn estimate(
        params: &Self::Params,
        bounds: &ReportedBounds,
        profile: &StandProfile,
    ) -> Result<Estimate, MissingBound>;

    /// The verdict: a pure function of the recorded trace and the criteria.
    fn verdict(trace: &Self::Trace, criteria: &Self::Criteria) -> Verdict;
}

/// One named, parameterised instance of a scenario.
#[derive(Clone, Debug)]
pub struct Instance<S: Spec> {
    pub name: InstanceName,
    pub params: S::Params,
}

impl<S: Spec> Instance<S> {
    pub const fn new(name: InstanceName, params: S::Params) -> Self {
        Self { name, params }
    }

    /// The instance's full name, `scenario/instance`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", S::ID, self.name)
    }
}

/// Reads a requirement's criteria with the scenario's own type, so that a
/// wrong threshold fails validation instead of a run.
pub fn criteria_from_json<S: Spec>(
    value: serde_json::Value,
) -> Result<S::Criteria, serde_json::Error> {
    serde_json::from_value(value)
}

/// Reads an instance's parameters with the scenario's own type.
pub fn params_from_json<S: Spec>(value: serde_json::Value) -> Result<S::Params, serde_json::Error> {
    serde_json::from_value(value)
}
