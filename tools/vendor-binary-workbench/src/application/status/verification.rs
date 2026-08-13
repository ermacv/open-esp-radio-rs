//! Project verification-suite configuration and last-run readiness.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Deserialize;

use super::model::{Component, Phase, Readiness};
use crate::verification::PROJECT_VERIFICATION_REPORT_SCHEMA;
use crate::{
    application::ProjectContext,
    project::VerificationWorkspacePaths,
    run_spec::{InputRole, RunSpec},
    verification::{dispositions, load_evidence_baseline, profiles},
};

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    let Some(workspace) = context.project.verification.as_ref() else {
        return Phase::collect(
            "verification",
            vec![Component::new("suites", Readiness::NotConfigured)],
        );
    };
    Phase::collect(
        "verification",
        vec![
            suite_configuration(workspace),
            suite_inputs(workspace, context.run_spec, context.project_path),
            last_report_component(context),
        ],
    )
}

pub(super) fn last_report_component(context: &ProjectContext<'_>) -> Component {
    let Some(workspace) = context.project.verification.as_ref() else {
        return Component::new("last-verification", Readiness::NotConfigured);
    };
    last_report(workspace, &context.project.id, context.project_path)
}

fn suite_inputs(
    workspace: &VerificationWorkspacePaths,
    run_spec: Option<&RunSpec>,
    project_path: &Path,
) -> Component {
    let Some(run_spec) = run_spec else {
        return Component::new("suite-inputs", Readiness::Incomplete)
            .diagnostic("verification suite artifact bindings are unavailable")
            .next_action(format!(
                "run `vendor-binary-workbench project inputs init --project {}`",
                project_path.display()
            ));
    };
    let configured = run_spec
        .inputs()
        .iter()
        .map(|input| input.role.clone())
        .collect::<BTreeSet<_>>();
    let mut required = BTreeSet::new();
    for suite in &workspace.suites {
        required.insert(suite.rust_artifact_role.clone());
        if let Some(role) = &suite.rust_companion_role {
            required.insert(role.clone());
        }
        for vendor in &suite.vendor {
            required.insert(InputRole::SourceArtifact(vendor.source.clone()));
        }
    }
    let missing = required
        .difference(&configured)
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if missing.is_empty() {
        Component::new("suite-inputs", Readiness::Ready).detail("required_roles", required.len())
    } else {
        Component::new("suite-inputs", Readiness::Incomplete)
            .detail("missing_roles", missing.clone())
            .diagnostic(format!(
                "verification suites require missing run-spec roles: {}",
                missing.join(", ")
            ))
            .next_action(format!(
                "bind the missing roles with `vendor-binary-workbench project inputs init --project {}`",
                project_path.display()
            ))
    }
}

fn suite_configuration(workspace: &VerificationWorkspacePaths) -> Component {
    let mut profile_names = BTreeSet::new();
    let mut profile_contracts = 0usize;
    let mut disposition_entries = 0usize;
    let mut baseline_entries = 0usize;
    for suite in &workspace.suites {
        for path in &suite.profiles {
            if !path.is_file() {
                return missing("verification profile", path);
            }
            let loaded = match profiles::load(path) {
                Ok(loaded) => loaded,
                Err(error) => {
                    return Component::new("suites", Readiness::Invalid)
                        .detail("suite", suite.id.clone())
                        .detail("path", path.display().to_string())
                        .diagnostic(error);
                }
            };
            for profile in loaded {
                if !profile_names.insert(profile.name.clone()) {
                    return Component::new("suites", Readiness::Invalid)
                        .detail("suite", suite.id.clone())
                        .diagnostic(format!(
                            "verification profile {:?} is defined more than once",
                            profile.name
                        ));
                }
                profile_contracts += 1;
            }
        }
        match dispositions::Manifest::load_all(&suite.dispositions) {
            Ok(Some(manifest)) => disposition_entries += manifest.entries().count(),
            Ok(None) => unreachable!("project loader requires disposition fragments"),
            Err(error) => {
                return Component::new("suites", Readiness::Invalid)
                    .detail("suite", suite.id.clone())
                    .diagnostic(error);
            }
        }
        let mut evidence = crate::verification::EvidenceSet::new();
        for path in &suite.evidence_baselines {
            if !path.is_file() {
                return missing("evidence baseline", path);
            }
            let loaded = match load_evidence_baseline(path) {
                Ok(loaded) => loaded,
                Err(error) => {
                    return Component::new("suites", Readiness::Invalid)
                        .detail("suite", suite.id.clone())
                        .detail("path", path.display().to_string())
                        .diagnostic(error);
                }
            };
            for ((source, symbol), kind) in loaded {
                if let Err(error) =
                    crate::verification::record_evidence(&mut evidence, &source, &symbol, kind)
                {
                    return Component::new("suites", Readiness::Invalid)
                        .detail("suite", suite.id.clone())
                        .diagnostic(error);
                }
            }
        }
        baseline_entries += evidence.len();
    }
    Component::new("suites", Readiness::Ready)
        .detail("suites", workspace.suites.len())
        .detail("profile_contracts", profile_contracts)
        .detail("disposition_entries", disposition_entries)
        .detail("baseline_entries", baseline_entries)
}

fn missing(kind: &str, path: &Path) -> Component {
    Component::new("suites", Readiness::Incomplete)
        .detail("path", path.display().to_string())
        .diagnostic(format!("{kind} {} is missing", path.display()))
}

#[derive(Deserialize)]
struct StoredAggregateReport {
    schema_version: u32,
    command: String,
    project: String,
    passed: bool,
    complete_project_run: bool,
    replacement_graph: StoredReplacementGraph,
    rust_component_index: StoredRustComponentIndex,
    suites: Vec<StoredSuiteReport>,
}

#[derive(Deserialize)]
struct StoredSuiteReport {
    id: String,
    artifacts: Vec<StoredArtifact>,
}

#[derive(Deserialize)]
struct StoredArtifact {
    role: String,
    path: PathBuf,
    sha256: String,
}

#[derive(Deserialize)]
struct StoredRustComponentIndex {
    summary: StoredRustComponentIndexSummary,
}

#[derive(Deserialize)]
struct StoredRustComponentIndexSummary {
    reviewed_components: usize,
    source_resolved: usize,
    source_ambiguous: usize,
    source_missing: usize,
    compiled_resolved: usize,
    compiled_missing: usize,
    dwarf_locations: usize,
    freshness_checked: usize,
    freshness_fresh: usize,
    freshness_stale: usize,
    freshness_unknown: usize,
    artifact_freshness_fresh: usize,
    artifact_freshness_stale: usize,
    artifact_freshness_unknown: usize,
}

#[derive(Deserialize)]
struct StoredReplacementGraph {
    summary: StoredReplacementSummary,
}

#[derive(Deserialize)]
struct StoredReplacementSummary {
    vendor_functions: usize,
    production_components: usize,
    behavioral_matches: usize,
    production_matches: usize,
    bounded_matches: usize,
    probe_only_matches: usize,
    unmapped_matches: usize,
    implemented_unqualified: usize,
}

fn last_report(
    workspace: &VerificationWorkspacePaths,
    project_id: &str,
    project_path: &Path,
) -> Component {
    if !workspace.report.is_file() {
        return Component::new("last-verification", Readiness::Incomplete)
            .detail("path", workspace.report.display().to_string())
            .diagnostic("project verification has not been executed")
            .next_action(format!(
                "run `vendor-binary-workbench project verify --project {}`",
                project_path.display()
            ));
    }
    let input = match fs::read_to_string(&workspace.report) {
        Ok(input) => input,
        Err(error) => {
            return Component::new("last-verification", Readiness::Invalid)
                .detail("path", workspace.report.display().to_string())
                .diagnostic(error);
        }
    };
    let report: StoredAggregateReport = match serde_json::from_str(&input) {
        Ok(report) => report,
        Err(error) => {
            return Component::new("last-verification", Readiness::Invalid)
                .detail("path", workspace.report.display().to_string())
                .diagnostic(error);
        }
    };
    if report.schema_version != PROJECT_VERIFICATION_REPORT_SCHEMA
        || report.command != "project verify"
        || report.project != project_id
    {
        return Component::new("last-verification", Readiness::Invalid)
            .detail("path", workspace.report.display().to_string())
            .diagnostic("project verification report has an incompatible identity or schema");
    }
    let expected_suite_ids = workspace
        .suites
        .iter()
        .map(|suite| suite.id.as_str())
        .collect::<BTreeSet<_>>();
    let reported_suite_ids = report
        .suites
        .iter()
        .map(|suite| suite.id.as_str())
        .collect::<BTreeSet<_>>();
    if !report.complete_project_run || reported_suite_ids != expected_suite_ids {
        return Component::new("last-verification", Readiness::Incomplete)
            .detail("path", workspace.report.display().to_string())
            .detail("expected_suites", expected_suite_ids.len())
            .detail("reported_suites", reported_suite_ids.len())
            .diagnostic(
                "aggregate verification report is partial or stale for the current suite set",
            )
            .next_action(format!(
                "run `vendor-binary-workbench project verify --project {}`",
                project_path.display()
            ));
    }
    let currency = match artifact_currency(&report.suites) {
        Ok(currency) => currency,
        Err(error) => {
            return Component::new("last-verification", Readiness::Invalid)
                .detail("path", workspace.report.display().to_string())
                .diagnostic(error);
        }
    };
    if !currency.stale.is_empty() || !currency.missing.is_empty() {
        return Component::new("last-verification", Readiness::Incomplete)
            .detail("path", workspace.report.display().to_string())
            .detail("passed", report.passed)
            .detail("fresh", false)
            .detail("checked_inputs", currency.checked)
            .detail("stale_inputs", currency.stale)
            .detail("missing_inputs", currency.missing)
            .diagnostic("project verification report no longer matches its recorded inputs")
            .next_action(format!(
                "run `vendor-binary-workbench project verify --project {}`",
                project_path.display()
            ));
    }
    Component::new(
        "last-verification",
        if report.passed {
            Readiness::Ready
        } else {
            Readiness::Incomplete
        },
    )
    .detail("path", workspace.report.display().to_string())
    .detail("suites", report.suites.len())
    .detail("passed", report.passed)
    .detail("fresh", true)
    .detail("checked_inputs", currency.checked)
    .detail(
        "vendor_functions",
        report.replacement_graph.summary.vendor_functions,
    )
    .detail(
        "production_components",
        report.replacement_graph.summary.production_components,
    )
    .detail(
        "behavioral_matches",
        report.replacement_graph.summary.behavioral_matches,
    )
    .detail(
        "production_matches",
        report.replacement_graph.summary.production_matches,
    )
    .detail(
        "bounded_matches",
        report.replacement_graph.summary.bounded_matches,
    )
    .detail(
        "probe_only_matches",
        report.replacement_graph.summary.probe_only_matches,
    )
    .detail(
        "unmapped_matches",
        report.replacement_graph.summary.unmapped_matches,
    )
    .detail(
        "implemented_unqualified",
        report.replacement_graph.summary.implemented_unqualified,
    )
    .detail(
        "component_source_resolved",
        report.rust_component_index.summary.source_resolved,
    )
    .detail(
        "component_source_ambiguous",
        report.rust_component_index.summary.source_ambiguous,
    )
    .detail(
        "component_source_missing",
        report.rust_component_index.summary.source_missing,
    )
    .detail(
        "component_compiled_resolved",
        report.rust_component_index.summary.compiled_resolved,
    )
    .detail(
        "component_compiled_missing",
        report.rust_component_index.summary.compiled_missing,
    )
    .detail(
        "component_dwarf_locations",
        report.rust_component_index.summary.dwarf_locations,
    )
    .detail(
        "component_freshness_checked",
        report.rust_component_index.summary.freshness_checked,
    )
    .detail(
        "component_freshness_fresh",
        report.rust_component_index.summary.freshness_fresh,
    )
    .detail(
        "component_freshness_stale",
        report.rust_component_index.summary.freshness_stale,
    )
    .detail(
        "component_freshness_unknown",
        report.rust_component_index.summary.freshness_unknown,
    )
    .detail(
        "artifact_freshness_fresh",
        report.rust_component_index.summary.artifact_freshness_fresh,
    )
    .detail(
        "artifact_freshness_stale",
        report.rust_component_index.summary.artifact_freshness_stale,
    )
    .detail(
        "artifact_freshness_unknown",
        report
            .rust_component_index
            .summary
            .artifact_freshness_unknown,
    )
    .detail(
        "component_total",
        report.rust_component_index.summary.reviewed_components,
    )
    .detail(
        "currency_check",
        "project verify --check replays suites and verifies artifact/evidence currency",
    )
}

#[derive(Debug, Default, Eq, PartialEq)]
struct ArtifactCurrency {
    checked: usize,
    stale: Vec<String>,
    missing: Vec<String>,
}

fn artifact_currency(suites: &[StoredSuiteReport]) -> Result<ArtifactCurrency, String> {
    let mut recorded = BTreeMap::<PathBuf, (String, String)>::new();
    for suite in suites {
        for artifact in &suite.artifacts {
            let label = format!("{}:{}", suite.id, artifact.role);
            if let Some((previous_digest, previous_label)) = recorded.get(&artifact.path) {
                if previous_digest != &artifact.sha256 {
                    return Err(format!(
                        "verification report records conflicting digests for {} ({previous_label} and {label})",
                        artifact.path.display()
                    ));
                }
                continue;
            }
            recorded.insert(artifact.path.clone(), (artifact.sha256.clone(), label));
        }
    }

    let mut currency = ArtifactCurrency {
        checked: recorded.len(),
        ..ArtifactCurrency::default()
    };
    for (path, (expected, label)) in recorded {
        if !path.is_file() {
            currency.missing.push(format!("{label}={}", path.display()));
            continue;
        }
        let actual = crate::artifact_sha256(&path).map_err(|error| {
            format!("cannot hash verification input {}: {error}", path.display())
        })?;
        if actual != expected {
            currency.stale.push(format!("{label}={}", path.display()));
        }
    }
    Ok(currency)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ProjectSpec;

    #[test]
    fn checked_project_exposes_parseable_verification_suites() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("workbench remains under tools");
        let project = ProjectSpec::load(
            &root.join("verification/vendor/targets/esp32s31/vendor-project.toml"),
        )
        .unwrap();
        let component = suite_configuration(project.verification.as_ref().unwrap());
        assert_eq!(component.status, Readiness::Ready, "{component:?}");
    }

    #[test]
    fn artifact_currency_deduplicates_inputs_and_detects_changes() {
        let directory = std::env::temp_dir().join(format!(
            "vendor-workbench-verification-currency-{}",
            std::process::id()
        ));
        fs::create_dir_all(&directory).unwrap();
        let input = directory.join("input.toml");
        fs::write(&input, "first").unwrap();
        let digest = crate::artifact_sha256(&input).unwrap();
        let suites = vec![StoredSuiteReport {
            id: "one".to_owned(),
            artifacts: vec![
                StoredArtifact {
                    role: "profile".to_owned(),
                    path: input.clone(),
                    sha256: digest.clone(),
                },
                StoredArtifact {
                    role: "same-profile".to_owned(),
                    path: input.clone(),
                    sha256: digest,
                },
            ],
        }];

        assert_eq!(
            artifact_currency(&suites).unwrap(),
            ArtifactCurrency {
                checked: 1,
                ..ArtifactCurrency::default()
            }
        );
        fs::write(&input, "second").unwrap();
        let currency = artifact_currency(&suites).unwrap();
        assert_eq!(currency.checked, 1);
        assert_eq!(currency.stale.len(), 1);
        fs::remove_dir_all(directory).unwrap();
    }
}
