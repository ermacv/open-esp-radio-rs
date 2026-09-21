//! Readiness diagnostics for project-owned linked-IR generation profiles.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::{
    artifacts::inspect_linked_ir,
    project::ProjectSpec,
    providers,
    run_spec::{InputRole, RunSpec},
    target::TargetSpec,
};

#[derive(Serialize)]
pub(crate) struct IrDoctorReport {
    pub(crate) status: &'static str,
    pub(crate) profiles: Vec<IrProfileReport>,
    pub(crate) coverage: Vec<crate::application::coverage::CoverageObligation>,
    pub(crate) errors: usize,
    pub(crate) warnings: usize,
}

impl IrDoctorReport {
    pub(crate) const fn counts(&self) -> (usize, usize) {
        (self.errors, self.warnings)
    }

    pub(crate) fn issues(&self) -> Vec<String> {
        let mut issues = self
            .coverage
            .iter()
            .flat_map(|item| {
                item.issues
                    .iter()
                    .map(|issue| format!("{}: {issue}", item.id))
            })
            .collect::<Vec<_>>();
        for profile in &self.profiles {
            if !profile.missing.is_empty() {
                issues.push(format!(
                    "linked IR profile {:?} is missing source bindings: {}",
                    profile.id,
                    profile.missing.join(", ")
                ));
            }
            if profile.output_status == "not-generated" {
                issues.push(format!(
                    "linked IR profile {:?} has not been generated ({})",
                    profile.id,
                    profile.output.display()
                ));
            }
            for diagnostic in &profile.diagnostics {
                issues.push(format!(
                    "linked IR profile {:?} {}: {}",
                    profile.id, diagnostic.kind, diagnostic.error
                ));
            }
        }
        issues
    }
}

#[derive(Serialize)]
pub(crate) struct IrProfileReport {
    pub(crate) id: String,
    pub(crate) input_status: &'static str,
    pub(crate) sources: Vec<String>,
    pub(crate) missing: Vec<String>,
    pub(crate) roots: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) symbol_prefix: Option<String>,
    pub(crate) include_reachable: bool,
    pub(crate) entry_contract: String,
    pub(crate) contract_status: &'static str,
    pub(crate) output_status: &'static str,
    pub(crate) functions: usize,
    pub(crate) decode_blockers: usize,
    pub(crate) registers: usize,
    pub(crate) field_candidates: usize,
    pub(crate) review_linked: bool,
    pub(crate) output: std::path::PathBuf,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub(crate) diagnostics: Vec<IrProfileDiagnostic>,
}

#[derive(Serialize)]
pub(crate) struct IrProfileDiagnostic {
    pub(crate) kind: &'static str,
    pub(crate) error: String,
}

pub(crate) fn inspect(
    project: &ProjectSpec,
    run_spec: Option<&RunSpec>,
    target: &TargetSpec,
) -> IrDoctorReport {
    let coverage = crate::application::coverage::inspect(project, run_spec);
    let coverage_errors = coverage.iter().filter(|item| !item.complete()).count();
    if project.ir_profiles.is_empty() {
        return IrDoctorReport {
            status: if coverage_errors == 0 {
                "not-configured"
            } else {
                "incomplete"
            },
            profiles: Vec::new(),
            coverage,
            errors: coverage_errors,
            warnings: 0,
        };
    }
    let available_sources = run_spec
        .into_iter()
        .flat_map(RunSpec::inputs)
        .filter_map(|input| match &input.role {
            InputRole::SourceArtifact(source) => Some(source.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let linked_outputs = project
        .registers
        .as_ref()
        .into_iter()
        .flat_map(|registers| &registers.review_ir_reports)
        .collect::<BTreeSet<_>>();
    let mut errors = coverage_errors;
    let mut warnings = 0usize;
    let mut profiles = Vec::with_capacity(project.ir_profiles.len());
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
        let mut diagnostics = Vec::new();
        if let Some(run) = run_spec
            && let Err(error) = super::project_inputs::resolve_inputs(profile, run)
                .and_then(|inputs| super::project_inputs::validate_ir_context(&inputs))
        {
            errors += 1;
            diagnostics.push(IrProfileDiagnostic {
                kind: "input-composition",
                error: error.to_string(),
            });
        }
        let contract_status = match providers::entry_contract_or_neutral(
            target.knowledge_provider.as_deref(),
            &profile.entry_contract,
        ) {
            Ok(_) => "ready",
            Err(error) => {
                errors += 1;
                diagnostics.push(IrProfileDiagnostic {
                    kind: "entry-contract",
                    error: error.to_string(),
                });
                "invalid"
            }
        };
        let (output_status, functions, decode_blockers, registers, fields) =
            if !profile.output.is_dir() {
                warnings += 1;
                ("not-generated", 0, 0, 0, 0)
            } else {
                match inspect_linked_ir(&profile.output) {
                    Ok(summary) => (
                        "ready",
                        summary.functions,
                        summary.decode_blockers,
                        summary.registers,
                        summary.field_candidates,
                    ),
                    Err(error) => {
                        errors += 1;
                        diagnostics.push(IrProfileDiagnostic {
                            kind: "output",
                            error: error.to_string(),
                        });
                        ("invalid", 0, 0, 0, 0)
                    }
                }
            };
        profiles.push(IrProfileReport {
            id: profile.id.clone(),
            input_status,
            sources: requested.into_iter().map(str::to_owned).collect(),
            missing: missing.into_iter().map(str::to_owned).collect(),
            roots: profile.roots.mode(),
            symbol_prefix: match &profile.roots {
                crate::project_ir::ProjectIrRoots::All => None,
                crate::project_ir::ProjectIrRoots::SymbolPrefix(prefix) => Some(prefix.clone()),
            },
            include_reachable: profile.include_reachable,
            entry_contract: profile.entry_contract.clone(),
            contract_status,
            output_status,
            functions,
            decode_blockers,
            registers,
            field_candidates: fields,
            review_linked: linked_outputs.contains(&profile.output),
            output: profile.output.clone(),
            diagnostics,
        });
    }
    IrDoctorReport {
        status: "configured",
        profiles,
        coverage,
        errors,
        warnings,
    }
}
