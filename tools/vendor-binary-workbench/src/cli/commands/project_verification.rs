//! Project-owned execution of independent vendor/Rust verification suites.

use std::{collections::BTreeSet, fmt::Write as _};

use super::super::{
    MmioMap, ProjectVerifyArgs, Result, SourcePath, SourceValue, VerifyInventoryArgs,
};
use crate::{
    TargetSpec,
    application::generated_file,
    project::{ProjectSpec, ProjectVerificationGate, VerificationSuiteSpec},
    run_spec::{InputRole, RunSpec},
    verification::{
        ProjectVerificationReport, ProjectVerificationSuiteReport, RustArtifactInput,
        RustComponentIndex, dispositions::Manifest, write_evidence_candidate,
    },
};

pub(super) fn run(
    arguments: ProjectVerifyArgs,
    project_manifest: &std::path::Path,
    project: &ProjectSpec,
    run_spec: Option<&RunSpec>,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<bool> {
    let report = execute(arguments, project_manifest, project, run_spec, svd, target)?;
    let passed = report.passed;
    let output = &project
        .verification
        .as_ref()
        .expect("verification workspace was checked by execute")
        .report;
    crate::cli::output::render_report(&report, || render_human(&report, output));
    Ok(passed)
}

pub(super) fn execute(
    arguments: ProjectVerifyArgs,
    project_manifest: &std::path::Path,
    project: &ProjectSpec,
    run_spec: Option<&RunSpec>,
    svd: &MmioMap,
    target: &TargetSpec,
) -> Result<ProjectVerificationReport> {
    let workspace = project
        .verification
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("project verify requires [[verification.suites]]"))?;
    let run_spec = run_spec.ok_or_else(|| {
        crate::Error::invalid("project verify requires a run spec with suite artifact bindings")
    })?;
    let selected = select_suites(&workspace.suites, &arguments.suite)?;
    let complete_project_run = selected.len() == workspace.suites.len();
    if arguments.check && !complete_project_run {
        return Err(crate::Error::invalid(
            "project verify --check cannot be combined with a partial --suite selection",
        ));
    }

    let rust_artifacts = selected
        .iter()
        .map(|suite| {
            Ok(RustArtifactInput {
                suite: suite.id.clone(),
                path: required_input(run_spec, &suite.rust_artifact_role, &suite.id)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    preflight_rust_artifacts(project_manifest, &selected, &rust_artifacts)?;

    let mut suites = Vec::with_capacity(selected.len());
    let mut passed = true;
    for suite in &selected {
        let arguments = suite_arguments(suite, run_spec)
            .map_err(|error| error.verification_suite(suite.id.clone()))?;
        let report = super::verify_inventory::execute(arguments, svd, target)
            .map_err(|error| error.verification_suite(suite.id.clone()))?;
        passed &= report.verification.passed;
        suites.push(ProjectVerificationSuiteReport {
            id: suite.id.clone(),
            verification: report,
        });
    }
    for suite in &suites {
        let path = crate::qualification::suite_report_path(&workspace.report, &suite.id);
        generated_file::write_or_check_json(
            &path,
            suite,
            arguments.check,
            "verification suite report",
            true,
        )?;
    }
    if let Some(directory) = arguments.candidate_evidence_dir.as_deref() {
        if !directory.is_dir() {
            return Err(crate::Error::invalid(format!(
                "project verification candidate directory does not exist: {}",
                directory.display()
            )));
        }
        for (suite, report) in selected.iter().zip(&suites) {
            let candidate = directory.join(format!("{}.toml", suite.id));
            let protected = suite
                .evidence_baselines
                .iter()
                .map(|path| ("accepted baseline", path.as_path()))
                .collect::<Vec<_>>();
            write_evidence_candidate(&candidate, &protected, &report.verification.evidence_set())?;
        }
    }
    let report = ProjectVerificationReport::new(
        project.id.clone(),
        passed,
        complete_project_run,
        suites,
        project_manifest,
        &rust_artifacts,
    )?;

    if complete_project_run {
        generated_file::write_or_check_json(
            &workspace.report,
            &report,
            arguments.check,
            "project verification report",
            true,
        )?;
    }
    Ok(report)
}

fn preflight_rust_artifacts(
    project_manifest: &std::path::Path,
    suites: &[&VerificationSuiteSpec],
    rust_artifacts: &[RustArtifactInput],
) -> Result<()> {
    let mut component_ids = BTreeSet::new();
    for suite in suites {
        let Some(manifest) = Manifest::load_all(&suite.dispositions)? else {
            continue;
        };
        component_ids.extend(manifest.entries().filter_map(|entry| {
            entry
                .rust_component
                .as_ref()
                .map(|component| component.label().to_owned())
        }));
    }
    let index =
        RustComponentIndex::build_component_ids(project_manifest, &component_ids, rust_artifacts)?;
    let stale = index.stale_components();
    let stale_artifacts = index.stale_artifacts();
    if stale.is_empty() && stale_artifacts.is_empty() {
        return Ok(());
    }
    let stale_artifacts = stale_artifacts
        .iter()
        .map(|artifact| artifact.path.as_str())
        .collect::<Vec<_>>();
    let stale_components = if stale.is_empty() {
        "none".to_owned()
    } else {
        stale.join(", ")
    };
    let stale_artifacts = if stale_artifacts.is_empty() {
        "none".to_owned()
    } else {
        stale_artifacts.join(", ")
    };
    Err(crate::Error::invalid(format!(
        "Rust verification artifact is older than its compiled source evidence (components: {}; artifacts: {}); rebuild the configured probe ELF before verification",
        stale_components, stale_artifacts,
    )))
}

fn select_suites<'a>(
    suites: &'a [VerificationSuiteSpec],
    selected: &[String],
) -> Result<Vec<&'a VerificationSuiteSpec>> {
    if selected.is_empty() {
        return Ok(suites.iter().collect());
    }
    let mut unique = BTreeSet::new();
    selected
        .iter()
        .map(|id| {
            if !unique.insert(id) {
                return Err(crate::Error::invalid(format!(
                    "project verify repeats suite {id:?}"
                )));
            }
            suites
                .iter()
                .find(|suite| suite.id == *id)
                .ok_or_else(|| crate::Error::invalid(format!("unknown verification suite {id:?}")))
        })
        .collect()
}

fn suite_arguments(
    suite: &VerificationSuiteSpec,
    run_spec: &RunSpec,
) -> Result<VerifyInventoryArgs> {
    let mut arguments = VerifyInventoryArgs {
        rust_artifact: Some(required_input(
            run_spec,
            &suite.rust_artifact_role,
            &suite.id,
        )?),
        rust_companion: suite
            .rust_companion_role
            .as_ref()
            .map(|role| required_input(run_spec, role, &suite.id))
            .transpose()?,
        rust_prefix: Some(suite.rust_prefix.clone()),
        profiles: suite.profiles.clone(),
        dispositions: suite.dispositions.clone(),
        evidence_baseline: suite.evidence_baselines.clone(),
        gate: match suite.gate {
            ProjectVerificationGate::Completion => "completion".to_owned(),
            ProjectVerificationGate::Regression { .. } => "regression".to_owned(),
        },
        match_floor: match suite.gate {
            ProjectVerificationGate::Completion => None,
            ProjectVerificationGate::Regression { match_floor } => Some(match_floor),
        },
        ..VerifyInventoryArgs::default()
    };
    for source in &suite.sources {
        arguments.source_artifact.push(SourcePath {
            source: source.clone(),
            path: required_input(
                run_spec,
                &InputRole::SourceArtifact(source.clone()),
                &suite.id,
            )?,
        });
        if let Some(path) = optional_input(run_spec, &InputRole::SourceInventory(source.clone())) {
            arguments.source_inventory.push(SourcePath {
                source: source.clone(),
                path,
            });
        }
        if let Some(path) = optional_input(run_spec, &InputRole::SourceCompanion(source.clone())) {
            arguments.source_companion.push(SourcePath {
                source: source.clone(),
                path,
            });
        }
        if let Some(prefix) = suite.source_prefixes.get(source) {
            arguments.source_prefix.push(SourceValue {
                source: source.clone(),
                value: prefix.clone(),
            });
        }
    }
    Ok(arguments)
}

fn required_input(run_spec: &RunSpec, role: &InputRole, suite: &str) -> Result<std::path::PathBuf> {
    optional_input(run_spec, role).ok_or_else(|| {
        crate::Error::invalid(format!(
            "verification suite {suite:?} requires run-spec role {role}"
        ))
    })
}

fn optional_input(run_spec: &RunSpec, role: &InputRole) -> Option<std::path::PathBuf> {
    run_spec
        .inputs()
        .iter()
        .find(|input| &input.role == role)
        .map(|input| input.path.clone())
}

fn render_human(report: &ProjectVerificationReport, output: &std::path::Path) {
    let mut text = String::new();
    let _ = writeln!(
        &mut text,
        "project verify: {}",
        if report.passed { "passed" } else { "failed" }
    );
    for suite in &report.suites {
        let core = &suite.verification.verification;
        let matched = suite
            .verification
            .sources
            .iter()
            .map(|source| source.summary.matched)
            .sum::<usize>();
        let incomplete = suite
            .verification
            .sources
            .iter()
            .map(|source| source.summary.incomplete)
            .sum::<usize>();
        let mismatched = suite
            .verification
            .sources
            .iter()
            .map(|source| source.summary.mismatched)
            .sum::<usize>();
        let limited_claims = suite
            .verification
            .sources
            .iter()
            .map(|source| source.summary.bounded_matches)
            .sum::<usize>();
        let limited = if limited_claims == 0 {
            String::new()
        } else {
            format!(", {limited_claims} bounded feature match(es)")
        };
        let _ = writeln!(
            &mut text,
            "  {}: {} ({} whole-function matched{limited}, {} mismatched, {} incomplete)",
            suite.id,
            if core.passed { "passed" } else { "failed" },
            matched,
            mismatched,
            incomplete
        );
    }
    let graph = &report.replacement_graph.summary;
    let _ = writeln!(
        &mut text,
        "  replacements: {} unique vendor functions, {} production components, {} behavioral matches",
        graph.vendor_functions, graph.production_components, graph.behavioral_matches
    );
    let _ = writeln!(
        &mut text,
        "  bindings: {} whole-function, {} bounded-feature, {} verification-probe; matches: {} production, {} probe-only, {} unmapped",
        graph.production_replacements,
        graph.production_feature_bindings,
        graph.verification_probe_bindings,
        graph.production_matches,
        graph.probe_only_matches,
        graph.unmapped_matches
    );
    if graph.bounded_matches != 0 {
        let _ = writeln!(
            &mut text,
            "  bounded features: {} production property match(es), not whole-function replacements",
            graph.bounded_matches,
        );
    }
    if graph.mismatches != 0 || graph.incomplete != 0 || graph.implemented_unqualified != 0 {
        let _ = writeln!(
            &mut text,
            "  qualification: {} mismatched, {} incomplete, {} limited claim(s) awaiting or using feature qualification",
            graph.mismatches, graph.incomplete, graph.implemented_unqualified
        );
    }
    let components = &report.rust_component_index.summary;
    let _ = writeln!(
        &mut text,
        "  component index: {}/{} source-resolved, {}/{} compiled, {} DWARF locations; component freshness {} fresh, {} stale, {} unknown; artifact freshness {} fresh, {} stale, {} unknown",
        components.source_resolved,
        components.reviewed_components,
        components.compiled_resolved,
        components.reviewed_components,
        components.dwarf_locations,
        components.freshness_fresh,
        components.freshness_stale,
        components.freshness_unknown,
        components.artifact_freshness_fresh,
        components.artifact_freshness_stale,
        components.artifact_freshness_unknown,
    );
    if report.complete_project_run {
        let _ = writeln!(&mut text, "  report: {}", output.display());
    } else {
        let _ = writeln!(
            &mut text,
            "  report: not written for partial suite selection"
        );
    }
    crate::cli::output::text(text);
}
