//! Compact, shareable evidence consumed by the qualification evaluator.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::Path,
};

use open_radio_vendor_semantics::VerificationClaim;
use serde::Serialize;
use sha2::{Digest, Sha256};

use super::{
    EvidenceClass, FunctionVerificationStatus, ProjectVerificationReport, RustComponentEvidence,
    VerificationEvidenceDocument,
};
use crate::Result;

#[path = "../../../../verification/vendor/schema/comparison.rs"]
mod comparison;

pub(crate) const VENDOR_EVIDENCE_INDEX_SCHEMA: u32 = 2;

#[derive(Debug, Serialize)]
pub(crate) struct VendorEvidenceIndex {
    pub(crate) schema_version: u32,
    pub(crate) command: &'static str,
    pub(crate) project: String,
    pub(crate) complete_project_run: bool,
    pub(crate) entries: Vec<VendorEvidenceEntry>,
    pub(crate) suite_states: BTreeMap<String, String>,
}

#[derive(Debug, Serialize)]
pub(crate) struct VendorEvidenceEntry {
    pub(crate) comparison: Option<comparison::Binding>,
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
                let comparison = if let Some(root) = repository_root {
                    let hashes = artifact_hashes
                        .iter()
                        .filter(|a| {
                            a.role.starts_with("source:")
                                || a.role.starts_with("auxiliary:")
                                || a.role == "rust-probes"
                        })
                        .map(|a| (a.role.clone(), a.sha256.clone()))
                        .collect();
                    let relative = canonical_project_manifest.strip_prefix(root).unwrap();
                    match comparison::current(
                        root,
                        relative,
                        &suite.id,
                        &function.source,
                        &function.vendor_symbol,
                        &hashes,
                    ) {
                        Ok(current) => {
                            if !current.public_artifacts_bound
                                || current.component != function.rust_component
                                || !identity
                                    .and_then(|i| i.identity.digest.as_ref())
                                    .is_some_and(|d| current.baseline_digests.contains(d))
                                || current
                                    .artifact_pins
                                    .iter()
                                    .any(|(role, hash)| hashes.get(role) != Some(hash))
                            {
                                release_blockers.push(
                                    "current comparison binding/baseline/artifact differs".into(),
                                );
                            }
                            let authenticated = suite
                                .verification
                                .verification
                                .model_context
                                .as_ref()
                                .map(|c| &c["authentication"]);
                            let authentication_matches = hashes.iter().all(|(role, hash)| {
                                let Some(source) = role
                                    .strip_prefix("source:")
                                    .and_then(|r| r.split_once(':'))
                                    .map(|(source, _)| source)
                                    .or_else(|| role.strip_prefix("auxiliary:"))
                                else {
                                    return true;
                                };
                                let identities = authenticated
                                    .and_then(serde_json::Value::as_array)
                                    .into_iter()
                                    .flatten()
                                    .filter(|a| a["source"] == source)
                                    .collect::<Vec<_>>();
                                identities.is_empty()
                                    || identities.iter().any(|a| a["sha256"] == *hash)
                            });
                            let model_matches = authentication_matches
                                && comparison::models::matches(
                                    &current.model_context,
                                    suite.verification.verification.model_context.as_ref(),
                                );
                            if !model_matches
                                || current.input_hashes.iter().any(|(role, hash)| {
                                    !artifact_hashes
                                        .iter()
                                        .any(|a| &a.role == role && &a.sha256 == hash)
                                })
                            {
                                release_blockers
                                    .push("comparison inputs changed after execution".into());
                                // The current identity describes unexecuted inputs. Keep
                                // the original artifacts, never attach that identity to
                                // the historical verdict even as a non-eligible row.
                                None
                            } else {
                                Some(comparison::Binding {
                                    project_manifest: relative.into(),
                                    sha256: current.sha256,
                                })
                            }
                        }
                        Err(error) => {
                            release_blockers
                                .push(format!("comparison identity unavailable: {error}"));
                            None
                        }
                    }
                } else {
                    release_blockers.push("comparison workspace unavailable".into());
                    None
                };
                entries.push(VendorEvidenceEntry {
                    comparison,
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
            complete_project_run: report.complete_project_run,
            suite_states: report
                .suites
                .iter()
                .map(|suite| (suite.id.clone(), "complete".into()))
                .collect(),
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
            suite_states: BTreeMap::new(),
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
            comparison: None,
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
    #[test]
    fn publication_does_not_attach_current_profile_to_an_earlier_verdict() {
        use crate::verification::*;
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path();
        fs::write(root.join("Cargo.toml"), "[workspace]\n").unwrap();
        let project = root.join("project.toml");
        fs::write(
            &project,
            "id = 'fixture'\ntarget-spec = 'target.toml'\nchip-pack = 'chip.toml'\nverification-addon = 'addon.toml'\n",
        )
        .unwrap();
        fs::write(root.join("addon.toml"), "model-inputs = 'model-inputs.json'\n[[suites]]\nid = 'suite'\nmodel-mechanisms = ['abi']\nprofiles = ['profile.toml']\ndispositions = ['disposition.toml']\n[[suites.vendor]]\nsource = 'vendor'\nall = true\n").unwrap();
        comparison::models::fixture(root);
        let profile = root.join("profile.toml");
        fs::write(
            &profile,
            "[[profiles]]\nvendor-source = 'vendor'\nvendor-symbol = 'entry'\nscope = 'A'\n",
        )
        .unwrap();
        let disposition = root.join("disposition.toml");
        fs::write(
            &disposition,
            "[[functions]]\nsource = 'vendor'\nsymbol = 'entry'\n",
        )
        .unwrap();
        let mut inputs = ExecutionInputs::new().unwrap();
        let mut captured_profile = profile.clone();
        let mut captured_disposition = disposition.clone();
        inputs.capture(&mut captured_profile).unwrap();
        inputs.capture(&mut captured_disposition).unwrap();
        let target = crate::TargetSpec {
            id: "fixture".into(),
            knowledge_provider: None,
            architecture: crate::target::Architecture::Riscv32,
            calling_convention: crate::target::CallingConvention::RiscvIlp32,
            endianness: crate::target::Endianness::Little,
            pointer_width: 32,
            rust_target: "riscv32imac-unknown-none-elf".into(),
        };
        let evidence = BTreeMap::from([(
            ("vendor".into(), "entry".into()),
            EvidenceIdentity::plain("fixture"),
        )]);
        let mut verification = verification_core_report(VerificationCoreInputs {
            target: &target,
            gate: crate::VerificationGate::Completion,
            summary: crate::VerifySummary::default(),
            orphan_probes: 0,
            evidence_baseline_passed: true,
            passed: true,
            evidence: &evidence,
            artifacts: &[
                ("profiles:0", captured_profile.as_path()),
                ("dispositions:0", captured_disposition.as_path()),
            ],
            release_gaps: &[],
        })
        .unwrap();
        verification.model_context = Some(
            comparison::current(
                root,
                Path::new("project.toml"),
                "suite",
                "vendor",
                "entry",
                &BTreeMap::new(),
            )
            .unwrap()
            .model_context,
        );
        let mut function =
            FunctionVerificationReport::new("vendor", "entry", FunctionVerificationStatus::Match);
        function.evidence_class = EvidenceClass::ProductionTrace;
        let mut command = VerificationCommandReport {
            schema_version: VERIFICATION_REPORT_SCHEMA,
            command: "verify inventory",
            verification,
            sources: vec![SourceVerificationReport {
                source: "vendor".into(),
                summary: crate::VerifySummary::default(),
                functions: vec![function],
            }],
            inventory: vec![],
            protocols: None,
            evidence_comparison: None,
            report: None,
        };
        inputs.restore_paths(&mut command);
        let report = ProjectVerificationReport {
            schema_version: PROJECT_VERIFICATION_REPORT_SCHEMA,
            command: "project verify",
            project: "fixture".into(),
            passed: true,
            complete_project_run: false,
            replacement_graph: ReplacementGraph::from_suites(&[]).unwrap(),
            rust_component_index: RustComponentIndex {
                schema_version: 1,
                summary: Default::default(),
                artifacts: vec![],
                components: vec![],
                diagnostics: vec![],
            },
            suites: vec![ProjectVerificationSuiteReport {
                id: "suite".into(),
                verification: command,
            }],
        };
        let before = VendorEvidenceIndex::build(&report, &project).unwrap();
        assert!(before.entries[0].comparison.is_some());
        fs::write(
            &profile,
            fs::read_to_string(&profile)
                .unwrap()
                .replace("scope = 'A'", "scope = 'B'"),
        )
        .unwrap();
        let after = VendorEvidenceIndex::build(&report, &project).unwrap();
        assert!(after.entries[0].comparison.is_none());
        assert!(!after.entries[0].release_eligible);
        assert!(
            after.entries[0]
                .release_blockers
                .iter()
                .any(|reason| reason == "comparison inputs changed after execution")
        );
        assert_eq!(
            after.entries[0].artifact_hashes,
            before.entries[0].artifact_hashes
        );
    }
}
