//! Human and stable machine-facing rendering for project-doctor reports.

use std::fmt::Write as _;
use tabled::{builder::Builder, settings::Style};

use crate::cli::args::OutputFormat;

use super::model::{CapabilityReport, DoctorReport, InputReport};

pub(super) fn render(report: &DoctorReport) {
    if crate::cli::output::structured("project-doctor", report) {
        return;
    }
    match crate::cli::output::format() {
        OutputFormat::Human => human(report),
        OutputFormat::Tsv => tsv(report),
        OutputFormat::Json | OutputFormat::Jsonl => {
            unreachable!("structured project-doctor output was already emitted")
        }
    }
}

fn human(report: &DoctorReport) {
    outputln!("Project doctor: {} — {}", report.status, report.project.id);
    outputln!("  manifest: {}", report.project.path.display());
    outputln!(
        "  target:   {} ({})",
        report.target.id,
        report.target.path.display()
    );
    let mut rows = Builder::default();
    rows.push_record(["Capability", "Status", "Details"]);
    for capability in &report.capabilities {
        rows.push_record([
            capability.name.to_owned(),
            capability.status.to_owned(),
            human_details(capability),
        ]);
    }
    let mut capabilities = rows.build();
    capabilities.with(Style::rounded());
    outputln!("Capabilities:\n{capabilities}");
    report.ir_build.render_human();
    report.function_workspace.render_human();
    outputln!("Inputs: {}", report.run_spec.status);
    if let Some(path) = report.run_spec.path.as_deref() {
        outputln!("  run spec: {}", path.display());
    } else if let Some(diagnostic) = report.run_spec.diagnostic {
        outputln!("  {diagnostic}");
    }
    for input in &report.inputs {
        outputln!(
            "  {:<28} {:<20} {}",
            input.role,
            input.status,
            input.path.display()
        );
    }
    for diagnostic in &report.diagnostics {
        outputln!("{}: {}", diagnostic.level, diagnostic.message);
    }
    outputln!(
        "Summary: {} — errors={} warnings={} inputs={} valid-inputs={}",
        report.status,
        report.errors,
        report.warnings,
        report.inputs.len(),
        report.valid_inputs
    );
}

fn human_details(capability: &CapabilityReport) -> String {
    capability
        .details
        .iter()
        .filter(|field| field.name != "paths")
        .take(4)
        .map(|field| format!("{}={}", field.name, field.value))
        .collect::<Vec<_>>()
        .join(" ")
}

fn tsv(report: &DoctorReport) {
    outputln!(
        "PROJECT\tid={}\tmanifest={}",
        report.project.id,
        report.project.path.display()
    );
    outputln!(
        "TARGET\tid={}\tspec={}",
        report.target.id,
        report.target.path.display()
    );
    for capability in &report.capabilities {
        print_capability_tsv(capability);
    }
    report.ir_build.render_tsv();
    report.function_workspace.render_tsv();
    for diagnostic in &report.diagnostics {
        outputln!(
            "DIAGNOSTIC\t{}\t{}",
            diagnostic.level,
            sanitize(&diagnostic.message)
        );
    }
    match report.run_spec.path.as_deref() {
        Some(path) => outputln!("RUN-SPEC\t{}", path.display()),
        None => outputln!(
            "RUN-SPEC\t{}\t{}",
            report.run_spec.status,
            report.run_spec.diagnostic.unwrap_or("-")
        ),
    }
    for input in &report.inputs {
        print_input_tsv(input);
    }
    outputln!(
        "SUMMARY\tstatus={}\terrors={}\twarnings={}\tinputs={}\tvalid-inputs={}",
        report.status,
        report.errors,
        report.warnings,
        report.inputs.len(),
        report.valid_inputs
    );
}

fn print_capability_tsv(capability: &CapabilityReport) {
    let mut line = format!("CAPABILITY\t{}\t{}", capability.name, capability.status);
    for field in &capability.details {
        let _ = write!(
            line,
            "\t{}={}",
            field.name,
            sanitize(&field.value.to_string())
        );
    }
    outputln!("{line}");
}

fn print_input_tsv(input: &InputReport) {
    let mut line = format!(
        "INPUT\trole={}\tstatus={}",
        sanitize(&input.role),
        input.status
    );
    for (name, value) in [
        ("container", input.container.map(str::to_owned)),
        ("objects", input.objects.map(|value| value.to_string())),
        (
            "skipped-members",
            input.skipped_members.map(|value| value.to_string()),
        ),
        (
            "symbol-facts",
            input.symbol_facts.map(|value| value.to_string()),
        ),
        (
            "code-definitions",
            input.code_definitions.map(|value| value.to_string()),
        ),
        (
            "exported-definitions",
            input.exported_definitions.map(|value| value.to_string()),
        ),
        ("undefined", input.undefined.map(|value| value.to_string())),
    ] {
        if let Some(value) = value {
            let _ = write!(line, "\t{name}={}", sanitize(&value));
        }
    }
    let _ = write!(line, "\tpath={}", input.path.display());
    if let Some(error) = input.error.as_deref() {
        let _ = write!(line, "\terror={}", sanitize(error));
    }
    outputln!("{line}");
}

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\t' | '\r' | '\n' => ' ',
            character => character,
        })
        .collect()
}
