//! Readiness diagnostics for project-owned linked-IR generation profiles.

use std::collections::BTreeSet;

use serde::Serialize;

use crate::{
    harnesses,
    project::ProjectSpec,
    project_ir_report::inspect_project_ir_report,
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
                "  {:<20} inputs={:<20} output={:<14} functions={} registers={} fields={}",
                profile.id,
                profile.input_status,
                profile.output_status,
                profile.functions,
                profile.registers,
                profile.field_candidates
            );
            for diagnostic in &profile.diagnostics {
                outputln!("    {}: {}", diagnostic.kind, diagnostic.error);
            }
        }
    }

    pub(super) fn render_tsv(&self) {
        for profile in &self.profiles {
            for diagnostic in &profile.diagnostics {
                outputln!(
                    "IR-PROFILE-DIAGNOSTIC\tid={}\tkind={}\terror={}",
                    profile.id,
                    diagnostic.kind,
                    diagnostic.error
                );
            }
            outputln!(
                "IR-PROFILE\tid={}\tinputs={}\tsources={}\tmissing={}\tprefix={}\treachable={}\tcontract={}\tcontract-status={}\toutput-status={}\tfunctions={}\tregisters={}\tfield-candidates={}\treview-linked={}\toutput={}\tpseudo-status={}\tpseudo={}",
                profile.id,
                profile.input_status,
                display_values(profile.sources.iter().map(String::as_str)),
                display_values(profile.missing.iter().map(String::as_str)),
                if profile.symbol_prefix.is_empty() {
                    "<all>"
                } else {
                    &profile.symbol_prefix
                },
                profile.include_reachable,
                profile.entry_contract,
                profile.contract_status,
                profile.output_status,
                profile.functions,
                profile.registers,
                profile.field_candidates,
                if profile.review_linked { "yes" } else { "no" },
                profile.output.display(),
                profile.pseudo_status,
                profile
                    .pseudo
                    .as_deref()
                    .map_or_else(|| "-".to_owned(), |path| path.display().to_string())
            );
        }
        outputln!(
            "CAPABILITY\tir-build\t{}\tprofiles={}\terrors={}\twarnings={}",
            self.status,
            self.profiles.len(),
            self.errors,
            self.warnings
        );
    }
}

#[derive(Serialize)]
struct IrProfileReport {
    id: String,
    input_status: &'static str,
    sources: Vec<String>,
    missing: Vec<String>,
    symbol_prefix: String,
    include_reachable: bool,
    entry_contract: String,
    contract_status: &'static str,
    output_status: &'static str,
    functions: usize,
    registers: usize,
    field_candidates: usize,
    review_linked: bool,
    output: std::path::PathBuf,
    pseudo_status: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pseudo: Option<std::path::PathBuf>,
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
            target.harness.as_deref(),
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
                    diagnostics.push(IrProfileDiagnostic {
                        kind: "output",
                        error: error.to_string(),
                    });
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
        profiles.push(IrProfileReport {
            id: profile.id.clone(),
            input_status,
            sources: requested.into_iter().map(str::to_owned).collect(),
            missing: missing.into_iter().map(str::to_owned).collect(),
            symbol_prefix: profile.symbol_prefix.clone(),
            include_reachable: profile.include_reachable,
            entry_contract: profile.entry_contract.clone(),
            contract_status,
            output_status,
            functions,
            registers,
            field_candidates: fields,
            review_linked: linked_outputs.contains(&profile.output),
            output: profile.output.clone(),
            pseudo_status,
            pseudo: profile.pseudo_rust.clone(),
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

fn display_values<'a>(values: impl IntoIterator<Item = &'a str>) -> String {
    let values = values.into_iter().collect::<Vec<_>>();
    if values.is_empty() {
        "-".to_owned()
    } else {
        values.join(",")
    }
}
