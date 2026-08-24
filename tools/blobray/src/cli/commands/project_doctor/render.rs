//! Task-first human rendering for project-doctor reports.

use super::model::{CapabilityReport, DoctorReport};
use crate::{
    application::{FollowUpRequirements, ProjectContext},
    cli::{output, table},
};

pub(super) fn render(report: &DoctorReport, context: &ProjectContext<'_>) {
    output::render_report(report, || human(report, context));
}

fn human(report: &DoctorReport, context: &ProjectContext<'_>) {
    outputln!("{}", output::heading("Project doctor"));
    outputln!("Project:  {}", report.project.id);
    outputln!("Manifest: {}", report.project.path.display());
    outputln!(
        "Target:   {} ({})",
        report.target.id,
        report.target.path.display()
    );

    let outcome = if report.errors != 0 {
        output::failure(format!(
            "BLOCKED — {} error(s), {} warning(s)",
            report.errors, report.warnings
        ))
    } else if report.warnings != 0 {
        output::warning(format!("VALID — {} warning(s)", report.warnings))
    } else {
        output::success("VALID — configuration and local inputs are usable")
    };
    outputln!("\n{outcome}");

    let mut issues = report.ir_build.issues();
    issues.extend(report.function_workspace.issues());
    issues.extend(report.diagnostics.iter().map(|item| item.message.clone()));
    issues.extend(
        report
            .capabilities
            .iter()
            .filter(|capability| matches!(capability.status, "invalid" | "failed" | "missing"))
            .map(|capability| {
                let details = human_details(capability);
                if details.is_empty() {
                    format!("{}: {}", capability.name, capability.status)
                } else {
                    format!("{}: {} ({details})", capability.name, capability.status)
                }
            }),
    );
    if let Some(diagnostic) = report.run_spec.diagnostic {
        issues.push(format!("local run spec: {diagnostic}"));
    }
    issues.extend(report.inputs.iter().filter_map(|input| {
        input.error.as_ref().map_or_else(
            || {
                (input.status == "missing")
                    .then(|| format!("input {} is missing: {}", input.role, input.path.display()))
            },
            |error| {
                Some(format!(
                    "input {} at {}: {error}",
                    input.role,
                    input.path.display()
                ))
            },
        )
    }));
    issues.sort();
    issues.dedup();
    if !issues.is_empty() {
        outputln!("\n{}", output::heading("Problems"));
        let limit = if output::details() { issues.len() } else { 10 };
        for (index, issue) in issues.iter().take(limit).enumerate() {
            outputln!("{}. {}", index + 1, sanitize(issue));
        }
        if issues.len() > limit {
            outputln!(
                "{} more problem(s); rerun with --details for complete evidence.",
                issues.len() - limit
            );
        }
    }

    if report.errors != 0 || report.warnings != 0 {
        outputln!("\n{}", output::heading("Next"));
        if report.run_spec.status != "available" {
            outputln!("1. {}", context.inputs_init_help_command());
            outputln!(
                "2. {} --details",
                context.follow_up_command("project files", FollowUpRequirements::TARGET)
            );
        } else if report.inputs.iter().any(|input| input.status == "missing") {
            outputln!("1. rebuild or restore these already-bound artifacts:");
            for input in report
                .inputs
                .iter()
                .filter(|input| input.status == "missing")
            {
                outputln!("   {} -> {}", input.role, input.path.display());
            }
            if let Some(path) = report.run_spec.path.as_deref() {
                outputln!(
                    "2. only if a path changed, recreate its binding in {} with `{}`",
                    path.display(),
                    context.inputs_init_help_command(),
                );
            }
            outputln!(
                "3. {}",
                context.follow_up_command("project status", FollowUpRequirements::RUN_SPEC)
            );
        } else {
            outputln!(
                "1. {} --details",
                context.follow_up_command("project files", FollowUpRequirements::TARGET)
            );
            outputln!(
                "2. {}",
                context.follow_up_command("project analyze", FollowUpRequirements::ANALYSIS)
            );
            outputln!(
                "3. {}",
                context.follow_up_command("project status", FollowUpRequirements::RUN_SPEC)
            );
        }
    }

    if output::details() {
        outputln!("\n{}", output::heading("Capabilities"));
        outputln!(
            "{}",
            table::render(
                ["Capability", "Status", "Details"],
                report.capabilities.iter().map(|capability| [
                    capability.name.to_owned(),
                    capability.status.to_owned(),
                    human_details(capability),
                ]),
            )
        );
        report.ir_build.render_human();
        report.function_workspace.render_human();

        outputln!("\n{}", output::heading("Inputs"));
        if let Some(path) = report.run_spec.path.as_deref() {
            outputln!("Run spec: {}", path.display());
        }
        if report.inputs.is_empty() {
            outputln!("No caller-owned artifacts are bound.");
        } else {
            outputln!(
                "{}",
                table::render(
                    ["Role", "Status", "Path"],
                    report.inputs.iter().map(|input| [
                        input.role.clone(),
                        input.status.to_owned(),
                        input.path.display().to_string(),
                    ]),
                )
            );
        }
    }
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

fn sanitize(value: &str) -> String {
    value
        .chars()
        .map(|character| match character {
            '\t' | '\r' | '\n' => ' ',
            character => character,
        })
        .collect()
}
