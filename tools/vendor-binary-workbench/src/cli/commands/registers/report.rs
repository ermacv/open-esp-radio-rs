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
        "  observed={} reviewed={} unreviewed={} model-only={} draft-fields={} IR-reports={}",
        report.observed,
        report.reviewed,
        report.unreviewed,
        report.model_only,
        report.draft_field_partitions,
        report.ir_reports
    );
}

pub(super) fn print_review_tsv(report: &RegisterReviewDocument<'_>) {
    outputln!(
        "REGISTER-REVIEW\tstatus={}\tobserved={}\treviewed={}\tunreviewed={}\tmodel-only={}\tdraft-field-partitions={}\tir-reports={}\tir-registers={}\tir-only-registers={}\tir-field-candidates={}\tpath={}",
        report.status,
        report.observed,
        report.reviewed,
        report.unreviewed,
        report.model_only,
        report.draft_field_partitions,
        report.ir_reports,
        report.ir_registers,
        report.ir_only_registers,
        report.ir_field_candidates,
        report.path.display()
    );
}

pub(super) fn print_model_human(report: &RegisterModelDocument<'_>) {
    outputln!(
        "Register model: {} — {}",
        report.status,
        report.model.display()
    );
    outputln!(
        "  schema={} peripherals={} fragments={} address-space={}",
        report.model_schema,
        report.peripherals,
        report.fragments,
        report.address_space
    );
}

pub(super) fn print_model_tsv(report: &RegisterModelDocument<'_>) {
    if let Some(input) = report.input {
        outputln!(
            "REGISTER-MODEL\tstatus={}\tschema={}\tperipherals={}\tfragments={}\tannotations={}\taddress-space={}\tinput={}\tmodel={}",
            report.status,
            report.model_schema,
            report.peripherals,
            report.fragments,
            report.annotations.unwrap_or(0),
            report.address_space,
            input.display(),
            report.model.display()
        );
    } else {
        outputln!(
            "REGISTER-MODEL\tstatus={}\tschema={}\tperipherals={}\tfragments={}\tobserved-registers={}\taddress-space={}\tmodel={}",
            report.status,
            report.model_schema,
            report.peripherals,
            report.fragments,
            report.observed_registers,
            report.address_space,
            report.model.display()
        );
    }
}

pub(super) fn print_workspace_human(report: &RegisterWorkspaceDocument<'_>) {
    outputln!(
        "Register workspace: {} — {}",
        report.status,
        report.model.display()
    );
    outputln!(
        "  observed={} reviewed={} ignored={} manual={} unreviewed={} fields={}",
        report.observed,
        report.reviewed,
        report.ignored,
        report.manual,
        report.unreviewed,
        report.fields
    );
}

pub(super) fn print_workspace_tsv(report: &RegisterWorkspaceDocument<'_>) {
    outputln!(
        "REGISTER-WORKSPACE\tstatus={}\tdeny-unreviewed={}\tformat={}\tranges={}\tobserved={}\treviewed={}\tignored={}\tmanual={}\tunreviewed={}\tfields={}\tfacts={}\tmodel={}",
        report.status,
        report.deny_unreviewed,
        report.format,
        report.ranges,
        report.observed,
        report.reviewed,
        report.ignored,
        report.manual,
        report.unreviewed,
        report.fields,
        report.facts.display(),
        report.model.display()
    );
    if let Some(pack) = &report.pac_api {
        outputln!(
            "PAC-API\tstatus=valid\tschema={}\toperations={}\tsources={}\tpack={}",
            pack.schema,
            pack.operations,
            pack.sources,
            pack.pack.display()
        );
    }
    if let Some(lints) = &report.lints {
        outputln!(
            "REGISTER-LINTS\tstatus=valid\tschema={}\tforbidden-field-name-substrings={}\tpack={}",
            lints.schema,
            lints.forbidden_field_name_substrings,
            lints.pack.display()
        );
    }
    if let Some(memory) = &report.memory {
        outputln!(
            "REGISTER-MEMORY\tstatus=valid\tregisters={}\tmmio-regions={}",
            memory.registers,
            memory.mmio_regions
        );
    }
    if let Some(evidence) = &report.evidence {
        outputln!(
            "REGISTER-EVIDENCE\tstatus=valid\tcatalogs={}\tconfidence-levels={}\tsources={}\tranges={}",
            evidence.catalogs,
            evidence.confidence_levels,
            evidence.sources,
            evidence.ranges
        );
    }
}
