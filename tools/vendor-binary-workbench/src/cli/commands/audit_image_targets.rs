//! Final-ELF direct control-flow target audit command.

use serde::Serialize;

use super::super::*;
use crate::direct_target_audit::{ForbiddenTargetRange, audit_direct_targets};

#[derive(Serialize)]
struct DirectTargetFindingReport {
    range: String,
    section: String,
    site: u32,
    target: u32,
}

#[derive(Serialize)]
struct DirectTargetAuditReport {
    schema_version: u32,
    command: &'static str,
    artifact: String,
    passed: bool,
    executable_sections: usize,
    executable_bytes: usize,
    decoded_instructions: usize,
    unsupported_instructions: usize,
    forbidden_targets: Vec<DirectTargetFindingReport>,
}

pub(super) fn run(arguments: ImageAuditArgs) -> Result<bool> {
    let artifact = arguments
        .artifact
        .ok_or("missing --artifact")
        .map_err(crate::Error::invalid)?;
    let ranges = arguments
        .forbid
        .into_iter()
        .map(|range| ForbiddenTargetRange {
            name: range.name,
            start: range.start,
            end: range.end,
        })
        .collect::<Vec<_>>();
    let audit = audit_direct_targets(&artifact, &ranges)?;
    let passed = audit.forbidden_targets.is_empty();
    let report = DirectTargetAuditReport {
        schema_version: 1,
        command: "image audit-targets",
        artifact: artifact.display().to_string(),
        passed,
        executable_sections: audit.executable_sections,
        executable_bytes: audit.executable_bytes,
        decoded_instructions: audit.decoded_instructions,
        unsupported_instructions: audit.unsupported_instructions,
        forbidden_targets: audit
            .forbidden_targets
            .into_iter()
            .map(|finding| DirectTargetFindingReport {
                range: finding.range,
                section: finding.section,
                site: finding.site,
                target: finding.target,
            })
            .collect(),
    };
    crate::cli::output::render_report(&report, || render_human(&report), || render_tsv(&report));
    Ok(passed)
}

fn render_human(report: &DirectTargetAuditReport) {
    outputln!(
        "Direct-target audit: {} — {}",
        if report.passed { "passed" } else { "failed" },
        report.artifact
    );
    outputln!(
        "  sections={} bytes={} instructions={} unsupported={} forbidden={}",
        report.executable_sections,
        report.executable_bytes,
        report.decoded_instructions,
        report.unsupported_instructions,
        report.forbidden_targets.len(),
    );
    for finding in &report.forbidden_targets {
        outputln!(
            "  {}: {} {:#010x} -> {:#010x}",
            finding.range,
            finding.section,
            finding.site,
            finding.target,
        );
    }
}

fn render_tsv(report: &DirectTargetAuditReport) {
    outputln!(
        "audit\tdirect-targets\t{}\tartifact={}\tsections={}\tbytes={}\tinstructions={}\tunsupported={}\tforbidden={}",
        if report.passed { "passed" } else { "failed" },
        report.artifact,
        report.executable_sections,
        report.executable_bytes,
        report.decoded_instructions,
        report.unsupported_instructions,
        report.forbidden_targets.len(),
    );
    for finding in &report.forbidden_targets {
        outputln!(
            "finding\tdirect-target\t{}\t{}\t{:#010x}\t{:#010x}",
            finding.range,
            finding.section,
            finding.site,
            finding.target,
        );
    }
}
