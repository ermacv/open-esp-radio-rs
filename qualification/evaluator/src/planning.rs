//! Read-only execution selection derived from the same capability decisions.
use crate::{Result, engineering::ProjectMap, hil::EvidenceStatus, model::WorkKind};
use serde::Serialize;
use sha2::{Digest, Sha256};

#[derive(Debug, Serialize)]
pub(crate) struct Plan {
    schema: u16,
    kind: &'static str,
    target: String,
    scope_sha256: String,
    obligations: Vec<Obligation>,
}
#[derive(Debug, Serialize)]
struct Obligation {
    capability: String,
    scenario: String,
    checks: Vec<String>,
    minimum_repetitions: u8,
    property_sha256: String,
    procedure_sha256: String,
    action: &'static str,
    reason: String,
    evidence: Option<String>,
}
impl Plan {
    pub(crate) fn from_map(map: &ProjectMap) -> Result<Self> {
        let target = map
            .target
            .clone()
            .ok_or("execution planning requires an evaluated program")?;
        let mut obligations = Vec::new();
        for entry in &map.entries {
            // Dependencies inform applicability and readiness. A focused request
            // does not authorize executing every prerequisite's experiment.
            if map.focus.as_ref().is_some_and(|id| id != &entry.id) {
                continue;
            }
            let Some(evidence) = &entry.evidence else {
                continue;
            };
            for decision in &evidence.hil_decisions {
                let requirement = entry
                    .hil_requirements
                    .iter()
                    .find(|r| r.scenario == decision.scenario)
                    .ok_or("HIL decision has no declared obligation")?;
                let property = decision
                    .property
                    .as_ref()
                    .ok_or("HIL decision has no property binding")?;
                let (action, reason) = if decision.status == EvidenceStatus::Satisfied {
                    (
                        "satisfied",
                        "Existing applicable evidence satisfies this obligation.",
                    )
                } else {
                    match decision.next_work() {
                        Some((WorkKind::AssessApplicability, reason)) => ("review", reason),
                        Some((WorkKind::InvestigateFailure, reason)) => ("investigate", reason),
                        Some((_, reason)) => ("run", reason),
                        None => {
                            return Err("unsatisfied obligation has no explained next work".into());
                        }
                    }
                };
                obligations.push(Obligation {
                    capability: entry.id.clone(),
                    scenario: decision.scenario.clone(),
                    checks: requirement.checks.clone(),
                    minimum_repetitions: requirement.minimum_repetitions,
                    property_sha256: property.sha256.clone(),
                    procedure_sha256: property
                        .procedure_sha256
                        .clone()
                        .ok_or("HIL decision has no executable procedure binding")?,
                    action,
                    reason: reason.into(),
                    evidence: decision.evidence.clone(),
                });
            }
        }
        obligations.sort_by(|a, b| (&a.capability, &a.scenario).cmp(&(&b.capability, &b.scenario)));
        // Bind requested scope independently of evidence availability. Code changes
        // re-evaluate applicability; changed promises require a new plan.
        let scope = obligations
            .iter()
            .map(|o| {
                (
                    &o.capability,
                    &o.scenario,
                    &o.checks,
                    o.minimum_repetitions,
                    &o.property_sha256,
                )
            })
            .collect::<Vec<_>>();
        let scope_sha256 = format!(
            "{:x}",
            Sha256::digest(serde_json::to_vec(&(&target, &map.focus, &scope))?)
        );
        Ok(Self {
            schema: 1,
            kind: "open-esp-radio-hil-selection",
            target,
            scope_sha256,
            obligations,
        })
    }
}
