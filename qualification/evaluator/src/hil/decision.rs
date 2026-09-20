//! Explain applicability separately from observation and obligation completion.
//!
//! The supported boundary is a complete scenario repetition set. Named checks
//! refine that obligation; they do not yet certify independently closed phases.
//! Cross-image transfer requires a property policy and is not inferred here.

use super::*;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(super) enum Exclusion {
    EvaluatorDirty,
    ProducerDirty,
    DifferentCommit,
    ReplaySubjectNotBound,
    SourceBindingNotEstablished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum EvidenceStatus {
    Satisfied,
    Missing,
    UnresolvedFailure,
}

impl EvidenceStatus {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Satisfied => "satisfied",
            Self::Missing => "missing",
            Self::UnresolvedFailure => "unresolved-failure",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
enum ObligationGap {
    ScenarioNotPassed,
    InsufficientRepetitions,
    RequiredChecksUnavailable,
    CurrentCriteriaNotMet,
    ControlNotSatisfied,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct ObservationDecision {
    observation_id: Option<String>,
    run_id: String,
    outcome: Outcome,
    repetition_outcomes: Vec<Outcome>,
    exclusions: Vec<Exclusion>,
    obligation_gaps: Vec<ObligationGap>,
    completion_seal: Option<CompletionSeal>,
    subject: Option<subject::ObservationSubject>,
    started_unix_millis: u64,
    failure: Option<serde_json::Value>,
    repetition_failures: Vec<Option<serde_json::Value>>,
    applicable: bool,
    review: Option<review::ReviewLink>,
    resolution: Option<review::ResolutionLink>,
}

/// Diagnostics are derived, never stored back into the immutable observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct EvidenceDecision {
    pub(crate) scenario: String,
    checks: Vec<String>,
    minimum_repetitions: u8,
    applicability_policy: &'static str,
    completion_boundary: &'static str,
    pub(crate) status: EvidenceStatus,
    pub(crate) evidence: Option<String>,
    observations: Vec<ObservationDecision>,
    control: Option<Box<EvidenceDecision>>,
    pub(crate) reviews: Vec<review::ReviewDecision>,
    pub(crate) property: Option<review::PropertyBinding>,
}

impl EvidenceDecision {
    pub(crate) fn attach_reviews(
        &mut self,
        root: &Path,
        document: &crate::model::CapabilityDocument,
        declarations: &BTreeMap<String, crate::model::CapabilityDocument>,
        catalog: &ScenarioCatalog,
        reviews: &[review::ReviewDecision],
    ) -> Result<()> {
        self.property = Some(review::property(
            document,
            declarations,
            &HilRequirement {
                scenario: self.scenario.clone(),
                checks: self.checks.clone(),
                minimum_repetitions: self.minimum_repetitions,
            },
            catalog,
            root,
        )?);
        self.reviews = reviews
            .iter()
            .filter(|r| {
                r.scenario == self.scenario
                    || catalog.control_for(&self.scenario) == Some(&r.scenario)
            })
            .cloned()
            .collect();
        if let Some(control) = &mut self.control {
            control.attach_reviews(root, document, declarations, catalog, reviews)?;
        }
        Ok(())
    }

    /// Original completed observations, including excluded historical inputs.
    pub(crate) fn observation_counts(&self) -> ObservationCounts {
        ObservationCounts {
            total: self.observations.len(),
            passed: self
                .observations
                .iter()
                .filter(|o| o.outcome == Outcome::Passed)
                .count(),
            failed: self
                .observations
                .iter()
                .filter(|o| {
                    o.outcome == Outcome::Failed || o.repetition_outcomes.contains(&Outcome::Failed)
                })
                .count(),
            excluded: self.observations.iter().filter(|o| !o.applicable).count(),
        }
    }

    /// Guidance only: never changes evidence eligibility or resolves a failure.
    pub(crate) fn next_work(&self) -> Option<(crate::model::WorkKind, &'static str)> {
        use crate::model::WorkKind;
        if self.reviews.iter().any(|r| r.status != "applied") {
            return Some((
                WorkKind::AssessApplicability,
                "An explicit applicability review no longer binds this property/build; inspect its review status before editing it or choosing a rerun.",
            ));
        }
        if self
            .control
            .as_ref()
            .is_some_and(|control| control.status == EvidenceStatus::UnresolvedFailure)
        {
            return Some((
                WorkKind::InvestigateFailure,
                "The required control has an unresolved applicable failure; inspect that decision before repeating the experiment.",
            ));
        }
        match self.status {
            EvidenceStatus::Satisfied => None,
            EvidenceStatus::UnresolvedFailure => Some((
                WorkKind::InvestigateFailure,
                "An applicable failure remains unresolved; another PASS does not close it.",
            )),
            EvidenceStatus::Missing
                if self.observations.iter().any(|o| !o.exclusions.is_empty()) =>
            {
                Some((
                    WorkKind::AssessApplicability,
                    "Recorded observations were excluded; inspect their reasons before choosing a rerun. No cross-image transfer is inferred.",
                ))
            }
            EvidenceStatus::Missing if !self.observations.is_empty() => Some((
                WorkKind::Recheck,
                "Recorded attempts do not complete this obligation; inspect missing checks, repetitions and controls.",
            )),
            EvidenceStatus::Missing => Some((
                WorkKind::Experiment,
                "No completed observation is indexed for this scenario; plan it on a compatible fixture.",
            )),
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Serialize)]
pub(crate) struct ObservationCounts {
    pub(crate) total: usize,
    pub(crate) passed: usize,
    pub(crate) failed: usize,
    pub(crate) excluded: usize,
}

impl HilEvidenceIndex {
    pub(crate) fn decision_for(
        &self,
        requirement: &HilRequirement,
        catalog: &ScenarioCatalog,
    ) -> EvidenceDecision {
        let control = catalog.control_for(&requirement.scenario);
        // Catalog validation forbids control chains. A failed current control
        // cannot be hidden by a passing pair in another invocation.
        let control_decision = control.map(|id| {
            Box::new(self.decision_for(
                &HilRequirement {
                    scenario: id.to_owned(),
                    checks: Vec::new(),
                    minimum_repetitions: requirement.minimum_repetitions,
                },
                catalog,
            ))
        });
        let control_satisfied = control_decision
            .as_ref()
            .is_none_or(|decision| decision.status == EvidenceStatus::Satisfied);
        let contracts = requirement
            .checks
            .iter()
            .map(|name| Some((name, catalog.checks.get(&requirement.scenario)?.get(name)?)))
            .collect::<Option<Vec<_>>>();
        let mut decision = EvidenceDecision {
            scenario: requirement.scenario.clone(),
            checks: requirement.checks.clone(),
            minimum_repetitions: requirement.minimum_repetitions,
            applicability_policy: "current-source-composition",
            completion_boundary: "scenario-repetition-set",
            status: EvidenceStatus::Missing,
            evidence: None,
            observations: Vec::new(),
            control: control_decision,
            reviews: Vec::new(),
            property: None,
        };
        let mut candidates = Vec::new();
        for observation in self
            .scenarios
            .get(&requirement.scenario)
            .into_iter()
            .flatten()
        {
            let mut gaps = Vec::new();
            if observation.outcome != Outcome::Passed {
                gaps.push(ObligationGap::ScenarioNotPassed);
            }
            if observation.repetitions < usize::from(requirement.minimum_repetitions) {
                gaps.push(ObligationGap::InsufficientRepetitions);
            }
            let assessments = contracts.as_ref().map(|contracts| {
                observation
                    .measurements
                    .iter()
                    .flat_map(|measurements| {
                        contracts
                            .iter()
                            .map(|(name, contract)| checks::assess(name, contract, measurements))
                    })
                    .collect::<Vec<_>>()
            });
            if observation.measurements.len() != observation.repetitions
                || assessments
                    .as_ref()
                    .is_none_or(|values| values.contains(&checks::Assessment::Unavailable))
            {
                gaps.push(ObligationGap::RequiredChecksUnavailable);
            }
            let criteria_failed = assessments
                .as_ref()
                .is_some_and(|values| values.contains(&checks::Assessment::Failed));
            if criteria_failed {
                gaps.push(ObligationGap::CurrentCriteriaNotMet);
            }
            if !control_satisfied
                || !control.is_none_or(|id| {
                    self.scenarios.get(id).is_some_and(|controls| {
                        controls.iter().any(|candidate| {
                            candidate.run_id == observation.run_id
                                && candidate.applicable()
                                && candidate.outcome == Outcome::Passed
                                && candidate.repetitions == observation.repetitions
                        })
                    })
                })
            {
                gaps.push(ObligationGap::ControlNotSatisfied);
            }
            if observation.applicable() {
                if (observation.outcome == Outcome::Failed
                    || observation.repetition_outcomes.contains(&Outcome::Failed)
                    || (observation.outcome == Outcome::Passed && criteria_failed))
                    && observation.resolution.is_none()
                {
                    // A later PASS alone cannot explain a failure; only an
                    // explicit, validated disposition can resolve it.
                    decision.status = EvidenceStatus::UnresolvedFailure;
                }
                if gaps.is_empty() {
                    candidates.push(observation);
                }
            }
            decision.observations.push(ObservationDecision {
                applicable: observation.applicable(),
                review: observation.review.clone(),
                resolution: observation.resolution.clone(),
                observation_id: observation.observation_id(&requirement.scenario),
                completion_seal: observation.completion_seal.clone(),
                subject: observation.subject.clone(),
                started_unix_millis: observation.started_unix_millis,
                failure: observation.failure.clone(),
                repetition_failures: observation.repetition_failures.clone(),
                run_id: observation.run_id.clone(),
                outcome: observation.outcome,
                repetition_outcomes: observation.repetition_outcomes.clone(),
                exclusions: observation.exclusions.clone(),
                obligation_gaps: gaps,
            });
        }
        if decision.status != EvidenceStatus::UnresolvedFailure
            && let Some(observation) = candidates
                .into_iter()
                .max_by_key(|entry| (entry.started_unix_millis, &entry.run_id))
        {
            let mut reference = format!(
                "hil:{}/{}:repetitions={}",
                observation.run_id, requirement.scenario, observation.repetitions
            );
            if let Some(control) = control {
                reference.push_str(&format!(":control={control}"));
            }
            if let Some(seal) = &observation.completion_seal
                && seal.path.starts_with("attempts")
            {
                reference.push_str(&format!(":attempt={}", seal.sha256));
            }
            if !requirement.checks.is_empty() {
                reference.push_str(&format!(":checks={}", requirement.checks.join(",")));
            }
            if let Some(review) = &observation.review {
                reference.push_str(&review.evidence_reference());
            }
            decision.status = EvidenceStatus::Satisfied;
            decision.evidence = Some(reference);
        }
        if decision
            .observations
            .iter()
            .any(|o| o.review.is_some() || o.resolution.is_some())
        {
            decision.applicability_policy = "reviewed-property";
        }
        decision
    }
}

#[cfg(test)]
pub(super) mod tests;
