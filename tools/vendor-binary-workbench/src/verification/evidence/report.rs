//! Typed verification report core and persistence.

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::{VerificationCommandReport, dispositions};
use super::EvidenceSet;
use crate::{Result, TargetSpec, VerificationGate, VerifySummary};

#[derive(Serialize)]
pub(crate) struct VerificationTargetDocument {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    harness: Option<String>,
    architecture: &'static str,
    calling_convention: &'static str,
    endianness: &'static str,
    pointer_width: u8,
    rust_target: String,
}

#[derive(Serialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub(crate) enum VerificationGateDocument {
    Completion,
    Regression { match_floor: usize },
}

#[derive(Serialize)]
pub(crate) struct VerificationSummaryDocument {
    vendor_functions: usize,
    matched: usize,
    symbolic_matches: usize,
    effect_contract_matches: usize,
    scenario_matches: usize,
    state_matches: usize,
    composition_matches: usize,
    mismatched: usize,
    incomplete: usize,
    missing: usize,
    implemented_unqualified: usize,
    not_yet_ported: usize,
    orphan_rust_probes: usize,
}

#[derive(Serialize)]
pub(crate) struct QualificationBlockerDocument {
    source: String,
    symbol: String,
}

#[derive(Serialize)]
pub(crate) struct QualificationGapDocument {
    source: String,
    symbol: String,
    rust_component: String,
    blocked_by: Vec<QualificationBlockerDocument>,
}

#[derive(Serialize)]
pub(crate) struct VerificationArtifactDocument {
    role: String,
    path: String,
    sha256: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerificationEvidenceDocument {
    pub(crate) source: String,
    pub(crate) symbol: String,
    pub(crate) kind: String,
}

#[derive(Serialize)]
pub(crate) struct VerificationCoreReport {
    target: VerificationTargetDocument,
    gate: VerificationGateDocument,
    pub(crate) passed: bool,
    pub(crate) evidence_baseline_passed: bool,
    summary: VerificationSummaryDocument,
    qualification_gaps: Vec<QualificationGapDocument>,
    artifacts: Vec<VerificationArtifactDocument>,
    evidence: Vec<VerificationEvidenceDocument>,
}

pub(crate) struct VerificationCoreInputs<'a, S> {
    pub(crate) target: &'a TargetSpec,
    pub(crate) gate: VerificationGate,
    pub(crate) summary: VerifySummary,
    pub(crate) orphan_probes: usize,
    pub(crate) evidence_baseline_passed: bool,
    pub(crate) passed: bool,
    pub(crate) evidence: &'a EvidenceSet,
    pub(crate) artifacts: &'a [(S, &'a Path)],
    pub(crate) qualification_gaps: &'a [&'a dispositions::Entry],
}

pub(crate) fn verification_core_report<S: AsRef<str>>(
    inputs: VerificationCoreInputs<'_, S>,
) -> Result<VerificationCoreReport> {
    let VerificationCoreInputs {
        target,
        gate,
        summary,
        orphan_probes,
        evidence_baseline_passed,
        passed,
        evidence,
        artifacts,
        qualification_gaps,
    } = inputs;
    Ok(VerificationCoreReport {
        target: VerificationTargetDocument {
            id: target.id.clone(),
            harness: target.harness.clone(),
            architecture: target.architecture.label(),
            calling_convention: target.calling_convention.label(),
            endianness: target.endianness.label(),
            pointer_width: target.pointer_width,
            rust_target: target.rust_target.clone(),
        },
        gate: match gate {
            VerificationGate::Completion => VerificationGateDocument::Completion,
            VerificationGate::Regression { match_floor } => {
                VerificationGateDocument::Regression { match_floor }
            }
        },
        passed,
        evidence_baseline_passed,
        summary: VerificationSummaryDocument {
            vendor_functions: summary.vendor_functions,
            matched: summary.matched,
            symbolic_matches: summary.symbolic_matches,
            effect_contract_matches: summary.effect_contract_matches,
            scenario_matches: summary.scenario_matches,
            state_matches: summary.state_matches,
            composition_matches: summary.composition_matches,
            mismatched: summary.mismatched,
            incomplete: summary.incomplete,
            missing: summary.missing,
            implemented_unqualified: summary.implemented_unqualified,
            not_yet_ported: summary.not_yet_ported,
            orphan_rust_probes: orphan_probes,
        },
        qualification_gaps: qualification_gaps
            .iter()
            .map(|gap| QualificationGapDocument {
                source: gap.source.clone(),
                symbol: gap.symbol.clone(),
                rust_component: gap
                    .rust_component
                    .clone()
                    .unwrap_or_else(|| "missing".to_owned()),
                blocked_by: gap
                    .qualification_blockers
                    .iter()
                    .map(|(source, symbol)| QualificationBlockerDocument {
                        source: source.clone(),
                        symbol: symbol.clone(),
                    })
                    .collect(),
            })
            .collect(),
        artifacts: artifacts
            .iter()
            .map(|(role, artifact)| {
                Ok(VerificationArtifactDocument {
                    role: role.as_ref().to_owned(),
                    path: artifact.display().to_string(),
                    sha256: format!("{:x}", Sha256::digest(fs::read(artifact)?)),
                })
            })
            .collect::<Result<Vec<_>>>()?,
        evidence: evidence
            .iter()
            .map(|((source, symbol), kind)| VerificationEvidenceDocument {
                source: source.clone(),
                symbol: symbol.clone(),
                kind: kind.clone(),
            })
            .collect(),
    })
}

pub(crate) fn write_verification_json_report(
    path: &Path,
    report: &VerificationCommandReport<'_>,
) -> Result<()> {
    fs::write(path, serde_json::to_string_pretty(report)? + "\n")?;
    Ok(())
}
