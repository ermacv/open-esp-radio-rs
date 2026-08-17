//! Typed verification report core and persistence.

use std::{
    fs,
    io::{BufWriter, Write},
    path::Path,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::super::{VerificationCommandReport, dispositions};
use super::{EvidenceIdentity, EvidenceSet};
use crate::{Result, TargetSpec, VerificationGate, VerifySummary};

#[derive(Serialize)]
pub(crate) struct VerificationTargetDocument {
    id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    knowledge_provider: Option<String>,
    architecture: &'static str,
    calling_convention: &'static str,
    endianness: &'static str,
    pointer_width: u8,
    rust_target: String,
}

#[derive(Serialize)]
#[serde(tag = "mode", rename_all = "kebab-case")]
pub(crate) enum VerificationGateDocument {
    Informational,
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
    bounded_matches: usize,
    mismatched: usize,
    incomplete: usize,
    missing: usize,
    implemented_unqualified: usize,
    not_yet_ported: usize,
    orphan_rust_probes: usize,
}

#[derive(Serialize)]
pub(crate) struct ReleaseBlockerDocument {
    source: String,
    symbol: String,
}

#[derive(Serialize)]
pub(crate) struct ReleaseGapDocument {
    source: String,
    symbol: String,
    rust_component: String,
    blocked_by: Vec<ReleaseBlockerDocument>,
}

#[derive(Serialize)]
pub(crate) struct VerificationArtifactDocument {
    pub(crate) role: String,
    path: String,
    pub(crate) sha256: String,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerificationEvidenceDocument {
    pub(crate) source: String,
    pub(crate) symbol: String,
    #[serde(flatten)]
    pub(crate) identity: EvidenceIdentity,
}

#[derive(Serialize)]
pub(crate) struct VerificationCoreReport {
    target: VerificationTargetDocument,
    gate: VerificationGateDocument,
    pub(crate) passed: bool,
    pub(crate) evidence_baseline_passed: bool,
    summary: VerificationSummaryDocument,
    release_gaps: Vec<ReleaseGapDocument>,
    pub(crate) artifacts: Vec<VerificationArtifactDocument>,
    pub(crate) evidence: Vec<VerificationEvidenceDocument>,
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
    pub(crate) release_gaps: &'a [&'a dispositions::Entry],
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
        release_gaps,
    } = inputs;
    Ok(VerificationCoreReport {
        target: VerificationTargetDocument {
            id: target.id.clone(),
            knowledge_provider: target.knowledge_provider.clone(),
            architecture: target.architecture.label(),
            calling_convention: target.calling_convention.label(),
            endianness: target.endianness.label(),
            pointer_width: target.pointer_width,
            rust_target: target.rust_target.clone(),
        },
        gate: match gate {
            VerificationGate::Informational => VerificationGateDocument::Informational,
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
            bounded_matches: summary.bounded_matches,
            mismatched: summary.mismatched,
            incomplete: summary.incomplete,
            missing: summary.missing,
            implemented_unqualified: summary.implemented_unqualified,
            not_yet_ported: summary.not_yet_ported,
            orphan_rust_probes: orphan_probes,
        },
        release_gaps: release_gaps
            .iter()
            .map(|gap| ReleaseGapDocument {
                source: gap.source.clone(),
                symbol: gap.symbol.clone(),
                rust_component: gap
                    .rust_component
                    .as_ref()
                    .map(|component| component.label().to_owned())
                    .unwrap_or_else(|| "missing".to_owned()),
                blocked_by: gap
                    .release_blockers
                    .iter()
                    .map(|(source, symbol)| ReleaseBlockerDocument {
                        source: source.clone(),
                        symbol: symbol.clone(),
                    })
                    .collect(),
            })
            .collect(),
        artifacts: artifacts
            .iter()
            .map(|(role, artifact)| {
                let canonical = fs::canonicalize(artifact)?;
                Ok(VerificationArtifactDocument {
                    role: role.as_ref().to_owned(),
                    path: canonical.display().to_string(),
                    sha256: format!("{:x}", Sha256::digest(fs::read(&canonical)?)),
                })
            })
            .collect::<Result<Vec<_>>>()?,
        evidence: evidence
            .iter()
            .map(
                |((source, symbol), identity)| VerificationEvidenceDocument {
                    source: source.clone(),
                    symbol: symbol.clone(),
                    identity: identity.clone(),
                },
            )
            .collect(),
    })
}

pub(crate) fn write_verification_json_report(
    path: &Path,
    report: &VerificationCommandReport,
) -> Result<()> {
    let mut writer = BufWriter::new(fs::File::create(path)?);
    serde_json::to_writer_pretty(&mut writer, report)?;
    writer.write_all(b"\n")?;
    writer.flush()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verification_artifacts_are_canonical_and_cwd_independent() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/../Cargo.toml");
        let target = TargetSpec {
            id: "fixture".to_owned(),
            knowledge_provider: None,
            architecture: crate::target::Architecture::Riscv32,
            calling_convention: crate::target::CallingConvention::RiscvIlp32,
            endianness: crate::target::Endianness::Little,
            pointer_width: 32,
            rust_target: "riscv32imac-unknown-none-elf".to_owned(),
        };
        let evidence = EvidenceSet::new();
        let report = verification_core_report(VerificationCoreInputs {
            target: &target,
            gate: VerificationGate::Completion,
            summary: VerifySummary::default(),
            orphan_probes: 0,
            evidence_baseline_passed: true,
            passed: true,
            evidence: &evidence,
            artifacts: &[("manifest", manifest.as_path())],
            release_gaps: &[],
        })
        .unwrap();
        assert_eq!(report.artifacts.len(), 1);
        assert_eq!(
            Path::new(&report.artifacts[0].path),
            fs::canonicalize(manifest).unwrap()
        );
    }
}
