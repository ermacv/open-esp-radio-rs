//! Compact, shareable evidence consumed by the qualification ledger.

use std::{collections::BTreeSet, fs, path::Path};

use open_radio_vendor_semantics::VerificationClaim;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    EvidenceClass, FunctionVerificationStatus, ProjectVerificationReport, RustComponentEvidence,
    VerificationEvidenceDocument,
};
use crate::Result;

pub(crate) const VENDOR_EVIDENCE_INDEX_SCHEMA: u32 = 1;

#[derive(Debug, Serialize)]
pub(crate) struct VendorEvidenceIndex {
    pub(crate) schema_version: u32,
    pub(crate) command: &'static str,
    pub(crate) project: String,
    pub(crate) complete_project_run: bool,
    pub(crate) entries: Vec<VendorEvidenceEntry>,
}

#[derive(Debug, Serialize)]
pub(crate) struct VendorEvidenceEntry {
    pub(crate) suite: String,
    pub(crate) source: String,
    pub(crate) symbol: String,
    pub(crate) evidence_class: EvidenceClass,
    pub(crate) status: FunctionVerificationStatus,
    pub(crate) release_eligible: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rust_component: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) rust_probe: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) claim: Option<VerificationClaim>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) evidence_kind: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) evidence_digest: Option<String>,
    pub(crate) baseline_passed: bool,
    pub(crate) artifact_hashes: Vec<EvidenceArtifactHash>,
    pub(crate) source_hashes: Vec<EvidenceSourceHash>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) release_blockers: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct EvidenceArtifactHash {
    pub(crate) role: String,
    pub(crate) sha256: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
pub(crate) struct EvidenceSourceHash {
    pub(crate) path: String,
    pub(crate) sha256: String,
}

impl VendorEvidenceIndex {
    pub(crate) fn build(
        report: &ProjectVerificationReport,
        project_manifest: &Path,
    ) -> Result<Self> {
        if !report.complete_project_run {
            return Err(crate::Error::invalid(
                "vendor evidence index requires a complete project verification run",
            ));
        }
        let canonical_project_manifest = fs::canonicalize(project_manifest)?;
        let repository_root = canonical_project_manifest
            .ancestors()
            .filter(|directory| directory.join("Cargo.toml").is_file())
            .last();
        let mut entries = Vec::new();
        for suite in &report.suites {
            let baseline_passed = suite.verification.verification.evidence_baseline_passed;
            let artifact_hashes = suite
                .verification
                .verification
                .artifacts
                .iter()
                .map(|artifact| EvidenceArtifactHash {
                    role: artifact.role.clone(),
                    sha256: artifact.sha256.clone(),
                })
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            for function in suite
                .verification
                .sources
                .iter()
                .flat_map(|source| &source.functions)
            {
                let component = function.rust_component.as_deref().and_then(|component| {
                    report
                        .rust_component_index
                        .components
                        .iter()
                        .find(|candidate| candidate.component_id == component)
                });
                let identity = evidence_identity(
                    &suite.verification.verification.evidence,
                    &function.source,
                    &function.vendor_symbol,
                );
                // The aggregate report retains every uncovered inventory row.
                // The qualification handoff is intentionally compact: keep
                // only reproducible evidence or an explicitly stronger
                // non-static evidence class.
                if identity.is_none() && function.evidence_class == EvidenceClass::StaticAnalysis {
                    continue;
                }
                let mut release_blockers = function.release_blockers.clone();
                if function.evidence_class != EvidenceClass::ProductionTrace {
                    release_blockers.push(format!(
                        "evidence class {} is supporting evidence only",
                        evidence_class_label(function.evidence_class)
                    ));
                }
                if !matches!(
                    function.status,
                    FunctionVerificationStatus::Match | FunctionVerificationStatus::BoundedMatch
                ) {
                    release_blockers.push(format!(
                        "verification status {} is not a successful production comparison",
                        function.status.label()
                    ));
                }
                if !baseline_passed {
                    release_blockers.push("accepted evidence baseline did not pass".to_owned());
                }
                if identity.is_none() {
                    release_blockers.push("no reproducible evidence identity".to_owned());
                }
                match component {
                    Some(component)
                        if component.source_status == "resolved"
                            && component.compiled_status == "resolved"
                            && component.freshness_status == "fresh" => {}
                    Some(component) => release_blockers.push(format!(
                        "production component is source={}, compiled={}, freshness={}",
                        component.source_status,
                        component.compiled_status,
                        component.freshness_status
                    )),
                    None => release_blockers
                        .push("reviewed production component is not resolved".to_owned()),
                }
                release_blockers.sort();
                release_blockers.dedup();
                let source_hashes = component
                    .map(|component| {
                        let repository_root = repository_root.ok_or_else(|| {
                            crate::Error::invalid(format!(
                                "cannot locate Cargo workspace above {}",
                                project_manifest.display()
                            ))
                        })?;
                        source_hashes(component, repository_root)
                    })
                    .transpose()?
                    .unwrap_or_default();
                entries.push(VendorEvidenceEntry {
                    suite: suite.id.clone(),
                    source: function.source.clone(),
                    symbol: function.vendor_symbol.clone(),
                    evidence_class: function.evidence_class,
                    status: function.status,
                    release_eligible: release_blockers.is_empty(),
                    rust_component: function.rust_component.clone(),
                    rust_probe: function.rust_symbol.clone(),
                    claim: function.claim,
                    evidence_kind: identity.map(|identity| identity.identity.kind.clone()),
                    evidence_digest: identity.and_then(|identity| identity.identity.digest.clone()),
                    baseline_passed,
                    artifact_hashes: artifact_hashes.clone(),
                    source_hashes,
                    release_blockers,
                });
            }
        }
        entries.sort_by(|left, right| {
            (&left.suite, &left.source, &left.symbol).cmp(&(
                &right.suite,
                &right.source,
                &right.symbol,
            ))
        });
        let index = Self {
            schema_version: VENDOR_EVIDENCE_INDEX_SCHEMA,
            command: "project verify vendor evidence index",
            project: report.project.clone(),
            complete_project_run: true,
            entries,
        };
        validate_shareable_index(&index, repository_root)?;
        Ok(index)
    }
}

fn validate_shareable_index(
    index: &VendorEvidenceIndex,
    repository_root: Option<&Path>,
) -> Result<()> {
    for hash in index.entries.iter().flat_map(|entry| {
        entry
            .artifact_hashes
            .iter()
            .map(|hash| hash.sha256.as_str())
            .chain(entry.source_hashes.iter().map(|hash| hash.sha256.as_str()))
    }) {
        if hash.len() != 64 || !hash.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(crate::Error::invalid(
                "vendor evidence index contains a malformed artifact/source digest",
            ));
        }
    }

    let encoded = serde_json::to_string(index)?;
    let private_artifact_marker = ["_oracles", "/"].concat();
    if encoded.contains(&private_artifact_marker)
        || repository_root.is_some_and(|root| encoded.contains(&root.display().to_string()))
    {
        return Err(crate::Error::invalid(
            "vendor evidence index contains a private or absolute repository path",
        ));
    }
    Ok(())
}

fn evidence_identity<'a>(
    evidence: &'a [VerificationEvidenceDocument],
    source: &str,
    symbol: &str,
) -> Option<&'a VerificationEvidenceDocument> {
    evidence
        .iter()
        .find(|entry| entry.source == source && entry.symbol == symbol)
}

fn source_hashes(
    component: &RustComponentEvidence,
    repository_root: &Path,
) -> Result<Vec<EvidenceSourceHash>> {
    component
        .source_items
        .iter()
        .map(|item| {
            let path = Path::new(&item.path);
            let relative = path.strip_prefix(repository_root).map_err(|_| {
                crate::Error::invalid(format!(
                    "production source {} is outside repository root {}",
                    path.display(),
                    repository_root.display()
                ))
            })?;
            Ok(EvidenceSourceHash {
                path: relative.display().to_string(),
                sha256: format!("{:x}", Sha256::digest(fs::read(path)?)),
            })
        })
        .collect::<Result<BTreeSet<_>>>()
        .map(BTreeSet::into_iter)
        .map(Iterator::collect)
}

const fn evidence_class_label(class: EvidenceClass) -> &'static str {
    match class {
        EvidenceClass::ProductionTrace => "production-trace",
        EvidenceClass::SharedCore => "shared-core",
        EvidenceClass::StaticAnalysis => "static-analysis",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty_index(project: &str) -> VendorEvidenceIndex {
        VendorEvidenceIndex {
            schema_version: VENDOR_EVIDENCE_INDEX_SCHEMA,
            command: "project verify vendor evidence index",
            project: project.to_owned(),
            complete_project_run: true,
            entries: Vec::new(),
        }
    }

    #[test]
    fn compact_index_rejects_private_and_absolute_repository_paths() {
        let private = ["_oracles", "/vendor.elf"].concat();
        assert!(validate_shareable_index(&empty_index(&private), None).is_err());
        assert!(
            validate_shareable_index(
                &empty_index("/workspace/open-radio/generated"),
                Some(Path::new("/workspace/open-radio")),
            )
            .is_err()
        );
        assert!(validate_shareable_index(&empty_index("esp32s31-radio"), None).is_ok());
    }

    #[test]
    fn compact_index_accepts_only_digest_shaped_hashes() {
        let mut index = empty_index("esp32s31-radio");
        index.entries.push(VendorEvidenceEntry {
            suite: "suite".to_owned(),
            source: "rom".to_owned(),
            symbol: "function".to_owned(),
            evidence_class: EvidenceClass::StaticAnalysis,
            status: FunctionVerificationStatus::Incomplete,
            release_eligible: false,
            rust_component: None,
            rust_probe: None,
            claim: None,
            evidence_kind: None,
            evidence_digest: None,
            baseline_passed: false,
            artifact_hashes: vec![EvidenceArtifactHash {
                role: "source:rom".to_owned(),
                sha256: "not-a-digest".to_owned(),
            }],
            source_hashes: Vec::new(),
            release_blockers: Vec::new(),
        });
        assert!(validate_shareable_index(&index, None).is_err());
    }
}
