//! Typed register-workspace lifecycle reports and presentation renderers.

use std::path::Path;

use serde::Serialize;

#[derive(Serialize)]
pub(super) struct RegisterReviewDocument<'a> {
    pub(super) schema: u32,
    pub(super) command: &'static str,
    pub(super) status: &'static str,
    pub(super) observed: usize,
    pub(super) reviewed: usize,
    pub(super) ignored: usize,
    pub(super) unreviewed: usize,
    pub(super) model_only: usize,
    pub(super) draft_field_partitions: usize,
    pub(super) ir_reports: usize,
    pub(super) ir_registers: usize,
    pub(super) ir_only_registers: usize,
    pub(super) ir_field_candidates: usize,
    pub(super) path: &'a Path,
}

#[derive(Serialize)]
pub(super) struct RegisterModelDocument<'a> {
    pub(super) schema: u32,
    pub(super) command: &'static str,
    pub(super) status: &'static str,
    pub(super) model_schema: u32,
    pub(super) peripherals: usize,
    pub(super) fragments: usize,
    pub(super) observed_registers: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) annotations: Option<usize>,
    pub(super) address_space: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) input: Option<&'a Path>,
    pub(super) model: &'a Path,
}

#[derive(Serialize)]
pub(super) struct RegisterWorkspaceDocument<'a> {
    pub(super) schema: u32,
    pub(super) command: &'static str,
    pub(super) status: &'static str,
    pub(super) deny_unreviewed: bool,
    pub(super) format: &'static str,
    pub(super) ranges: usize,
    pub(super) observed: usize,
    pub(super) reviewed: usize,
    pub(super) ignored: usize,
    pub(super) manual: usize,
    pub(super) unreviewed: usize,
    pub(super) fields: usize,
    pub(super) facts: &'a Path,
    pub(super) model: &'a Path,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) pac_api: Option<PacApiDocument<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) lints: Option<RegisterLintDocument<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) memory: Option<RegisterMemoryDocument>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(super) evidence: Option<RegisterEvidenceDocument>,
}

#[derive(Serialize)]
pub(super) struct PacApiDocument<'a> {
    pub(super) schema: u32,
    pub(super) operations: usize,
    pub(super) sources: usize,
    pub(super) pack: &'a Path,
}

#[derive(Serialize)]
pub(super) struct RegisterLintDocument<'a> {
    pub(super) schema: u32,
    pub(super) forbidden_field_name_substrings: usize,
    pub(super) pack: &'a Path,
}

#[derive(Serialize)]
pub(super) struct RegisterMemoryDocument {
    pub(super) registers: usize,
    pub(super) mmio_regions: usize,
}

#[derive(Serialize)]
pub(super) struct RegisterEvidenceDocument {
    pub(super) catalogs: usize,
    pub(super) confidence_levels: usize,
    pub(super) sources: usize,
    pub(super) ranges: usize,
}

pub(super) fn print_review_human(report: &RegisterReviewDocument<'_>) {
    outputln!(
        "Register review: {} — {}",
        report.status,
        report.path.display()
    );
    outputln!(
        "{}",
        crate::cli::table::render(
            ["Metric", "Count"],
            [
                ["Observed registers".into(), report.observed.to_string()],
                ["Reviewed registers".into(), report.reviewed.to_string()],
                [
                    "Outside publication scope".into(),
                    report.ignored.to_string()
                ],
                ["Unreviewed registers".into(), report.unreviewed.to_string()],
                ["Model-only registers".into(), report.model_only.to_string()],
                [
                    "Draft field partitions".into(),
                    report.draft_field_partitions.to_string(),
                ],
                ["Linked-IR reports".into(), report.ir_reports.to_string()],
                [
                    "Linked-IR registers".into(),
                    report.ir_registers.to_string()
                ],
                [
                    "Linked-IR-only registers".into(),
                    report.ir_only_registers.to_string(),
                ],
                [
                    "Linked-IR field candidates".into(),
                    report.ir_field_candidates.to_string(),
                ],
            ],
        )
    );
}

pub(super) fn print_model_human(report: &RegisterModelDocument<'_>) {
    outputln!(
        "Register model: {} — {}",
        report.status,
        report.model.display()
    );
    outputln!(
        "{}",
        crate::cli::table::render(
            [
                "Schema",
                "Peripherals",
                "Fragments",
                "Observed",
                "Address space"
            ],
            [[
                report.model_schema.to_string(),
                report.peripherals.to_string(),
                report.fragments.to_string(),
                report.observed_registers.to_string(),
                report.address_space.to_owned(),
            ]],
        )
    );
}

pub(super) fn print_workspace_human(report: &RegisterWorkspaceDocument<'_>) {
    outputln!(
        "Register workspace: {} — {}",
        report.status,
        report.model.display()
    );
    outputln!(
        "Coverage:\n{}",
        crate::cli::table::render(
            [
                "Observed",
                "Reviewed",
                "Ignored",
                "Manual",
                "Unreviewed",
                "Fields"
            ],
            [[
                report.observed.to_string(),
                report.reviewed.to_string(),
                report.ignored.to_string(),
                report.manual.to_string(),
                report.unreviewed.to_string(),
                report.fields.to_string(),
            ]],
        )
    );
    let mut checks = Vec::new();
    if let Some(pack) = &report.pac_api {
        checks.push([
            "PAC API".into(),
            "valid".into(),
            format!(
                "schema={} operations={} sources={}",
                pack.schema, pack.operations, pack.sources
            ),
        ]);
    }
    if let Some(lints) = &report.lints {
        checks.push([
            "Register lints".into(),
            "valid".into(),
            format!(
                "schema={} forbidden-name-substrings={}",
                lints.schema, lints.forbidden_field_name_substrings
            ),
        ]);
    }
    if let Some(memory) = &report.memory {
        checks.push([
            "Memory map".into(),
            "valid".into(),
            format!(
                "registers={} MMIO-regions={}",
                memory.registers, memory.mmio_regions
            ),
        ]);
    }
    if let Some(evidence) = &report.evidence {
        checks.push([
            "Evidence".into(),
            "valid".into(),
            format!(
                "catalogs={} confidence-levels={} sources={} ranges={}",
                evidence.catalogs, evidence.confidence_levels, evidence.sources, evidence.ranges
            ),
        ]);
    }
    if !checks.is_empty() {
        outputln!(
            "Checks:\n{}",
            crate::cli::table::render(["Check", "Status", "Details"], checks)
        );
    }
}
