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
    /// A bound's measurement is missing from a repetition, or recorded
    /// with another identity: the run cannot join the judged set.
    BoundMeasurementsUnavailable,
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

/// The bounds judged on every complete current run of the scenario.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
struct BoundFindings {
    runs: Vec<String>,
    findings: Vec<bound::Finding>,
}

/// Diagnostics are derived, never stored back into the immutable observation.
#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct EvidenceDecision {
    pub(crate) scenario: String,
    checks: Vec<String>,
    minimum_repetitions: u8,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    bounds: Vec<String>,
    /// What each bound found on every complete current run together, and
    /// those runs.
    #[serde(skip_serializing_if = "Option::is_none")]
    bound_findings: Option<BoundFindings>,
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
                "Recorded attempts do not complete this obligation; inspect missing checks, bounds, samples and repetitions.",
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
            bounds: requirement.bounds.iter().map(ToString::to_string).collect(),
            bound_findings: None,
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
            if !requirement.bounds.is_empty()
                && (observation.measurements.len() != observation.repetitions
                    || !requirement
                        .bounds
                        .iter()
                        .all(|bound| bound.available(&observation.measurements)))
            {
                gaps.push(ObligationGap::BoundMeasurementsUnavailable);
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
        if decision.status == EvidenceStatus::UnresolvedFailure
            || decision.investigation
            || candidates.is_empty()
        {
            return decision;
        }
        let latest = candidates
            .iter()
            .max_by_key(|entry| (entry.started_unix_millis, &entry.run_id))
            .copied()
            .expect("there is a candidate");
        let mut reference = if requirement.bounds.is_empty() {
            let mut reference = format!(
                "hil:{}/{}:repetitions={}",
                latest.run_id, requirement.scenario, latest.repetitions
            );
            if let Some(seal) = &latest.completion_seal
                && seal.path.starts_with("attempts")
            {
                reference.push_str(&format!(":attempt={}", seal.sha256));
            }
            reference
        } else {
            // Bounds are judged on every complete current run together, never
            // on a selected one, so rerunning proves nothing by luck.
            candidates.sort_by_key(|entry| (entry.started_unix_millis, &entry.run_id));
            let repetitions = candidates
                .iter()
                .flat_map(|entry| entry.measurements.iter().cloned())
                .collect::<Vec<_>>();
            let findings = requirement
                .bounds
                .iter()
                .map(|bound| bound.assess(&repetitions))
                .collect::<Vec<_>>();
            let met = findings
                .iter()
                .all(|finding| finding.assessment == bound::Assessment::Passed);
            let runs = candidates
                .iter()
                .map(|entry| entry.run_id.clone())
                .collect::<Vec<_>>();
            let mut reference = format!(
                "hil:{}/{}:repetitions={}",
                runs.join("+"),
                requirement.scenario,
                repetitions.len()
            );
            // Each independently sealed attempt the bounds rest on, by run.
            let attempts = candidates
                .iter()
                .filter_map(|entry| {
                    let seal = entry.completion_seal.as_ref()?;
                    seal.path
                        .starts_with("attempts")
                        .then(|| format!("{}={}", entry.run_id, seal.sha256))
                })
                .collect::<Vec<_>>();
            if !attempts.is_empty() {
                reference.push_str(&format!(":attempts={}", attempts.join(",")));
            }
            decision.bound_findings = Some(BoundFindings { runs, findings });
            if !met {
                return decision;
            }
            reference
        };
        if !requirement.checks.is_empty() {
            reference.push_str(&format!(":checks={}", requirement.checks.join(",")));
        }
        if !requirement.bounds.is_empty() {
            let bounds = requirement.bounds.iter().map(ToString::to_string);
            reference.push_str(&format!(":bounds={}", bounds.collect::<Vec<_>>().join(",")));
        }
        decision.status = EvidenceStatus::Satisfied;
        decision.evidence = Some(reference);
        decision
    }
}

#[cfg(test)]
pub(super) mod tests;
