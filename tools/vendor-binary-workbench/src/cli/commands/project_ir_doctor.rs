//! Readiness diagnostics for project-owned linked-IR generation profiles.

use std::collections::BTreeSet;

use crate::{
    harnesses, project::ProjectSpec, project_ir_report::inspect_project_ir_report,
    run_spec::RunSpec, target::TargetSpec,
};

pub(super) fn inspect(
    project: &ProjectSpec,
    run_spec: Option<&RunSpec>,
    target: &TargetSpec,
) -> (usize, usize) {
    if project.ir_profiles.is_empty() {
        outputln!("CAPABILITY\tir-build\tnot-configured");
        return (0, 0);
    }
    let available_sources = run_spec
        .into_iter()
        .flat_map(RunSpec::inputs)
        .filter_map(|(role, _)| role.strip_prefix("source-artifact:"))
        .collect::<BTreeSet<_>>();
    let linked_outputs = project
        .registers
        .as_ref()
        .into_iter()
        .flat_map(|registers| &registers.review_ir_reports)
        .collect::<BTreeSet<_>>();
    let mut errors = 0usize;
    let mut warnings = 0usize;
    for profile in &project.ir_profiles {
        let requested = if profile.sources.is_empty() {
            available_sources.clone()
        } else {
            profile
                .sources
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
        };
        let missing = requested
            .difference(&available_sources)
            .copied()
            .collect::<Vec<_>>();
        let input_status = if run_spec.is_none() {
            "run-spec-unavailable"
        } else if requested.is_empty() {
            errors += 1;
            "no-source-artifacts"
        } else if missing.is_empty() {
            "ready"
        } else {
            errors += 1;
            "missing-sources"
        };
        let contract_status = match harnesses::entry_contract_or_neutral(
            target.harness.as_deref(),
            &profile.entry_contract,
        ) {
            Ok(_) => "ready",
            Err(error) => {
                errors += 1;
                outputln!(
                    "IR-PROFILE-DIAGNOSTIC\tid={}\tkind=entry-contract\terror={error}",
                    profile.id
                );
                "invalid"
            }
        };
        let (output_status, functions, registers, fields) = if !profile.output.is_file() {
            warnings += 1;
            ("not-generated", 0, 0, 0)
        } else {
            match inspect_project_ir_report(&profile.output) {
                Ok(summary) => (
                    "ready",
                    summary.functions,
                    summary.registers,
                    summary.field_candidates,
                ),
                Err(error) => {
                    errors += 1;
                    outputln!(
                        "IR-PROFILE-DIAGNOSTIC\tid={}\tkind=output\terror={error}",
                        profile.id
                    );
                    ("invalid", 0, 0, 0)
                }
            }
        };
        let pseudo_status = match profile.pseudo_rust.as_deref() {
            None => "not-configured",
            Some(path) if path.is_file() => "ready",
            Some(_) => {
                warnings += 1;
                "not-generated"
            }
        };
        outputln!(
            "IR-PROFILE\tid={}\tinputs={}\tsources={}\tmissing={}\tprefix={}\treachable={}\tcontract={}\tcontract-status={}\toutput-status={}\tfunctions={}\tregisters={}\tfield-candidates={}\treview-linked={}\toutput={}\tpseudo-status={}\tpseudo={}",
            profile.id,
            input_status,
            display_values(requested.iter().copied()),
            display_values(missing.iter().copied()),
            if profile.symbol_prefix.is_empty() {
                "<all>"
            } else {
                &profile.symbol_prefix
            },
            profile.include_reachable,
            profile.entry_contract,
            contract_status,
            output_status,
            functions,
            registers,
            fields,
            if linked_outputs.contains(&profile.output) {
                "yes"
            } else {
                "no"
            },
            profile.output.display(),
            pseudo_status,
            profile
                .pseudo_rust
                .as_deref()
                .map_or_else(|| "-".to_owned(), |path| path.display().to_string())
        );
    }
    outputln!(
        "CAPABILITY\tir-build\tconfigured\tprofiles={}\terrors={}\twarnings={}",
        project.ir_profiles.len(),
        errors,
        warnings
    );
    (errors, warnings)
}

fn display_values<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let values = values.into_iter().collect::<Vec<_>>();
    if values.is_empty() {
        "-".to_owned()
    } else {
        values.join(",")
    }
}
