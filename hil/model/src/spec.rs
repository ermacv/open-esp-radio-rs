//! A scenario's typed specification and its instances' identity.

use core::fmt::Debug;

use serde::{Serialize, de::DeserializeOwned};

use crate::{
    Bounds, Estimate, InstanceName, PurposeKind, ReportedBounds, Resources, ScenarioId,
    ScenarioInstance, Sha256, StandProfile, TargetCapabilities, Verdict, WaitError,
    digest::canonical_json,
};

/// The contract a scenario exercises. A qualification proof is accepted
/// only when its scenario's subject is the capability's contract, so an
/// incidental scenario cannot stand in for the minimal one.
pub trait Subject {
    /// The contract's path, `crate::Trait`.
    const CONTRACT: &'static str;

    /// The contract's declared bounds, used while no run of an image's
    /// recipe has reported its actual ones; every wait on them is then
    /// provisional.
    fn declared_bounds() -> ReportedBounds;
}

/// Everything about a scenario but its execution: identity, purpose,
/// typed parameters, criteria and trace, what it needs and how long it
/// takes, and its verdict. Execution is a separate trait in the executor,
/// so this one stays a format the qualification evaluator links to
/// recompute verdicts.
pub trait Spec {
    const ID: ScenarioId;
    /// [`Conformance`](crate::Conformance), [`Interop`](crate::Interop),
    /// [`Measurement`](crate::Measurement) or an
    /// [`Investigation`](crate::Investigation); the executor admits the
    /// scenario by it.
    type Purpose: PurposeKind;
    type Subject: Subject;
    /// One instance's parameters; instances replace copied scenarios.
    type Params: Clone + Debug + Serialize + DeserializeOwned;
    /// Acceptance thresholds a qualification requirement owns. A bound the
    /// backend reports is not a criterion: it travels in the trace.
    type Criteria: Clone + Debug + Serialize + DeserializeOwned;
    /// What a run records; the verdict's only input besides the criteria.
    type Trace: Debug + Serialize + DeserializeOwned;

    /// Boards by role, fixtures and board capabilities.
    fn resources(params: &Self::Params) -> Resources;

    /// The composition and agent capabilities the image must serve.
    fn target(params: &Self::Params) -> TargetCapabilities;

    /// One instance's time, from its waits (its boot to Hello included)
    /// and its repetitions.
    fn estimate(
        params: &Self::Params,
        bounds: &Bounds,
        profile: &StandProfile,
    ) -> Result<Estimate, WaitError>;

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

    pub fn id(&self) -> ScenarioInstance {
        ScenarioInstance {
            scenario: S::ID,
            instance: self.name.clone(),
        }
    }

    /// SHA-256 over the canonical JSON ([`canonical_json`]) of the
    /// scenario, the instance, its parameters and the criteria. The verdict
    /// code's identity is the source closure the run's source snapshot
    /// records; together they pin what a recorded verdict was computed by.
    pub fn identity(&self, criteria: &S::Criteria) -> Result<Sha256, serde_json::Error> {
        let value = serde_json::json!({
            "scenario": S::ID,
            "instance": self.name,
            "params": serde_json::to_value(&self.params)?,
            "criteria": serde_json::to_value(criteria)?,
        });
        Ok(Sha256::of(&canonical_json(&value)))
    }
}
