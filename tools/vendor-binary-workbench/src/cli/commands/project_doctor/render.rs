//! Human rendering for project-doctor reports.

use tabled::{builder::Builder, settings::Style};

use super::model::{CapabilityReport, DoctorReport};

pub(super) fn render(report: &DoctorReport) {
    if crate::cli::output::structured(report) {
        return;
    }
    human(report);
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
