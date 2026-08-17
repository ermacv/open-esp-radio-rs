//! Readiness diagnostics for project-owned linked-IR generation profiles.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::{
    artifacts::inspect_linked_ir,
    harnesses,
    project::ProjectSpec,
    run_spec::{InputRole, RunSpec},
    target::TargetSpec,
};

#[derive(Serialize)]
pub(super) struct IrDoctorReport {
    status: &'static str,
    profiles: Vec<IrProfileReport>,
    errors: usize,
    warnings: usize,
}

impl IrDoctorReport {
    pub(super) const fn counts(&self) -> (usize, usize) {
        (self.errors, self.warnings)
    }

    pub(super) fn render_human(&self) {
        outputln!(
            "Linked IR: {} — profiles={} errors={} warnings={}",
            self.status,
            self.profiles.len(),
            self.errors,
            self.warnings
        );
        for profile in &self.profiles {
            outputln!(
                "  {:<20} roots={:<20} inputs={:<20} output={:<14} functions={} decode-blockers={} registers={} fields={}",
                profile.id,
                profile.symbol_prefix.as_ref().map_or_else(
                    || profile.roots.to_owned(),
                    |prefix| format!("{}:{prefix}", profile.roots),
                ),
                profile.input_status,
                profile.output_status,
                profile.functions,
                profile.decode_blockers,
                profile.registers,
                profile.field_candidates
            );
            for diagnostic in &profile.diagnostics {
                outputln!("    {}: {}", diagnostic.kind, diagnostic.error);
            }
        }
    }

    pub(super) fn issues(&self) -> Vec<String> {
        let mut issues = Vec::new();
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
struct IrProfileReport {
    id: String,
    input_status: &'static str,
    sources: Vec<String>,
    missing: Vec<String>,
    roots: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    symbol_prefix: Option<String>,
    include_reachable: bool,
    entry_contract: String,
    contract_status: &'static str,
    output_status: &'static str,
    functions: usize,
    decode_blockers: usize,
    registers: usize,
    field_candidates: usize,
    review_linked: bool,
    output: std::path::PathBuf,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    diagnostics: Vec<IrProfileDiagnostic>,
}

#[derive(Serialize)]
struct IrProfileDiagnostic {
    kind: &'static str,
    error: String,
}

pub(super) fn inspect(
    project: &ProjectSpec,
    run_spec: Option<&RunSpec>,
    target: &TargetSpec,
) -> IrDoctorReport {
    if project.ir_profiles.is_empty() {
        return IrDoctorReport {
            status: "not-configured",
            profiles: Vec::new(),
            errors: 0,
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
    let mut errors = 0usize;
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
        let contract_status = match harnesses::entry_contract_or_neutral(
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
        errors,
        warnings,
    }
}
