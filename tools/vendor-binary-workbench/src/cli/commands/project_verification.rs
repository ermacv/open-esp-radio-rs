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
        write_evidence_candidate,
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

    let mut suites = Vec::with_capacity(selected.len());
    let mut passed = true;
    for suite in &selected {
        let report =
            super::verify_inventory::execute(suite_arguments(suite, run_spec)?, svd, target)?;
        passed &= report.verification.passed;
        suites.push(ProjectVerificationSuiteReport {
            id: suite.id.clone(),
            verification: report,
        });
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
    let rust_artifacts = selected
        .iter()
        .map(|suite| {
            Ok(RustArtifactInput {
                suite: suite.id.clone(),
                path: required_input(run_spec, &suite.rust_artifact_role, &suite.id)?,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let report = ProjectVerificationReport::new(
        project.id.clone(),
        passed,
        complete_project_run,
        suites,
        project_manifest,
        &rust_artifacts,
    )?;

    if complete_project_run {
        let contents = serde_json::to_string_pretty(&report)? + "\n";
        generated_file::write_or_check(
            &workspace.report,
            &contents,
            arguments.check,
            "project verification report",
        )?;
    }
    crate::cli::output::render_report(&report, || render_human(&report, &workspace.report));
    Ok(passed)
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
        let _ = writeln!(
            &mut text,
            "  {}: {} ({} matched, {} mismatched, {} incomplete)",
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
        "  bindings: {} production-mapped, {} verification-probe-bound; matches: {} production, {} probe-only, {} unmapped",
        graph.production_replacements,
        graph.verification_probe_bindings,
        graph.production_matches,
        graph.probe_only_matches,
        graph.unmapped_matches
    );
    if graph.mismatches != 0 || graph.incomplete != 0 || graph.implemented_unqualified != 0 {
        let _ = writeln!(
            &mut text,
            "  qualification: {} mismatched, {} incomplete, {} implemented but unqualified",
            graph.mismatches, graph.incomplete, graph.implemented_unqualified
        );
    }
    let components = &report.rust_component_index.summary;
    let _ = writeln!(
        &mut text,
        "  component index: {}/{} source-resolved, {}/{} compiled, {} DWARF locations",
        components.source_resolved,
        components.reviewed_components,
        components.compiled_resolved,
        components.reviewed_components,
        components.dwarf_locations
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
