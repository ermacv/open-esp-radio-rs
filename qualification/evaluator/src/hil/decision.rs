//! Explain applicability separately from observation and obligation completion.
//!
//! The supported boundary is a complete scenario repetition set. Named checks
//! refine that obligation; they do not yet certify independently closed phases.
//! Only current observations count; cross-image transfer is never inferred.

use super::*;
use serde::Serialize;
use std::borrow::Cow;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum Exclusion {
    EvaluatorDirty,
    ProducerDirty,
    DifferentCommit,
    ReplaySubjectNotBound,
    SourceBindingNotEstablished,
    ProcedureMismatch,
    ObserverIdentityNotEstablished,
    /// The observer's recorded package graph, from an older runner, does not
    /// project onto today's observer inputs.
    ObserverGraphNotProjectable,
    CurrentObserverConfigurationUnavailable,
    /// The run's source snapshot is no longer the checkout's tree.
    SnapshotDiffersFromCheckout,
}

impl Exclusion {
    /// Whether the exclusion comes only from comparing the whole tree: the
    /// run's commit or its snapshot differ from the checkout. An observation
    /// whose build binds its sources otherwise ([`ScenarioEvidence`]'s
    /// `stale_snapshot`) is still recordable when every source its shard
    /// binds matches its snapshot. A dirty producer, a replay, an unbound
    /// build, a procedure or an observer mismatch never is.
    pub(crate) fn is_tree_binding(&self) -> bool {
        matches!(
            self,
            Self::DifferentCommit | Self::SnapshotDiffersFromCheckout | Self::EvaluatorDirty
        )
    }

    /// The exclusion's kebab-case identifier.
    pub(crate) fn id(&self) -> &'static str {
        match self {
            Self::EvaluatorDirty => "evaluator-dirty",
            Self::ProducerDirty => "producer-dirty",
            Self::DifferentCommit => "different-commit",
            Self::ReplaySubjectNotBound => "replay-subject-not-bound",
            Self::SourceBindingNotEstablished => "source-binding-not-established",
            Self::ProcedureMismatch => "procedure-mismatch",
            Self::ObserverIdentityNotEstablished => "observer-identity-not-established",
            Self::ObserverGraphNotProjectable => "observer-graph-not-projectable",
            Self::CurrentObserverConfigurationUnavailable => {
                "current-observer-configuration-unavailable"
            }
            Self::SnapshotDiffersFromCheckout => "snapshot-differs-from-checkout",
        }
    }
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
    /// The SHA-256 of the scenario's current normalized document, when the
    /// catalog defines it.
    pub(crate) procedure_sha256: Option<String>,
    observations: Vec<ObservationDecision>,
    /// Why the current firmware cannot run the scenario, as its catalog
    /// document declares; the obligation stays open until it can.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) unsupported: Option<String>,
    /// The requirement names an investigation scenario: no observation of
    /// it satisfies a qualification program.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub(crate) investigation: bool,
}

impl EvidenceDecision {
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
    pub(crate) fn next_work(&self) -> Option<(crate::model::WorkKind, Cow<'_, str>)> {
        use crate::model::WorkKind;
        if self.investigation {
            return Some((
                WorkKind::Implement,
                "The requirement names an investigation scenario, which cannot satisfy a qualification program; re-home it to a qualification scenario on a product image.".into(),
            ));
        }
        if let (EvidenceStatus::Missing, Some(reason)) = (self.status, &self.unsupported) {
            return Some((
                WorkKind::Implement,
                format!("Not runnable on the current firmware: {reason}").into(),
            ));
        }
        self.observed_work()
            .map(|(kind, reason)| (kind, reason.into()))
    }

    fn observed_work(&self) -> Option<(crate::model::WorkKind, &'static str)> {
        use crate::model::WorkKind;
        match self.status {
            EvidenceStatus::Satisfied => None,
            EvidenceStatus::UnresolvedFailure => Some((
                WorkKind::InvestigateFailure,
                "A current failure remains; another PASS does not close it.",
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
                "Recorded attempts do not complete this obligation; inspect missing checks and repetitions.",
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
            procedure_sha256: catalog
                .definitions
                .get(&requirement.scenario)
                .map(|document| {
                    oer_durable::sha256_bytes(procedure::normalize(document).to_string().as_bytes())
                }),
            observations: Vec::new(),
            unsupported: catalog
                .unsupported(&requirement.scenario)
                .map(str::to_owned),
            investigation: catalog.investigation(&requirement.scenario),
        };
        let mut candidates = Vec::new();
        for observation in self
            .scenarios
            .get(&requirement.scenario)
            .into_iter()
            .flatten()
        {
            let mut exclusions = observation.exclusions.clone();
            let procedure_matches =
                procedure::matches(observation, requirement, catalog).unwrap_or(false);
            if !procedure_matches {
                exclusions.push(Exclusion::ProcedureMismatch);
            }
            let applicable = observation.applicable() && procedure_matches;
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
            if applicable {
                if observation.outcome == Outcome::Failed
                    || observation.repetition_outcomes.contains(&Outcome::Failed)
                    || (observation.outcome == Outcome::Passed && criteria_failed)
                {
                    // A later PASS cannot explain a current failure.
                    decision.status = EvidenceStatus::UnresolvedFailure;
                }
                if gaps.is_empty() {
                    candidates.push(observation);
                }
            }
            decision.observations.push(ObservationDecision {
                applicable,
                observation_id: observation.observation_id(&requirement.scenario),
                completion_seal: observation.completion_seal.clone(),
                subject: observation.subject.clone(),
                started_unix_millis: observation.started_unix_millis,
                failure: observation.failure.clone(),
                repetition_failures: observation.repetition_failures.clone(),
                run_id: observation.run_id.clone(),
                outcome: observation.outcome,
                repetition_outcomes: observation.repetition_outcomes.clone(),
                exclusions,
                obligation_gaps: gaps,
            });
        }
        if decision.status != EvidenceStatus::UnresolvedFailure
            && !decision.investigation
            && let Some(observation) = candidates
                .into_iter()
                .max_by_key(|entry| (entry.started_unix_millis, &entry.run_id))
        {
            let mut reference = format!(
                "hil:{}/{}:repetitions={}",
                observation.run_id, requirement.scenario, observation.repetitions
            );
            if let Some(seal) = &observation.completion_seal
                && seal.path.starts_with("attempts")
            {
                reference.push_str(&format!(":attempt={}", seal.sha256));
            }
            if !requirement.checks.is_empty() {
                reference.push_str(&format!(":checks={}", requirement.checks.join(",")));
            }
            decision.status = EvidenceStatus::Satisfied;
            decision.evidence = Some(reference);
        }
        decision
    }
}

#[cfg(test)]
pub(super) mod tests;
