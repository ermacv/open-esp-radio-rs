//! Task-first human rendering for project-doctor reports.

use super::model::{CapabilityReport, DoctorReport};
use crate::{
    Result,
    application::{FollowUpStep, ProjectContext, ProjectContextRequirement},
    cli::{output, table},
};

pub(super) fn next_steps(
    report: &DoctorReport,
    context: &ProjectContext<'_>,
) -> Result<Vec<FollowUpStep>> {
    if report.errors == 0 && report.warnings == 0 {
        return Ok(Vec::new());
    }

    if report.run_spec.status != "available" {
        return Ok(vec![
            FollowUpStep::command(
                "Define caller-owned input bindings.",
                context.inputs_init_help_action()?,
            ),
            FollowUpStep::command(
                "Inspect the prerequisite-ordered project file contract.",
                context.follow_up_action(
                    ["project", "files", "--details"],
                    ProjectContextRequirement::Target,
                )?,
            ),
        ]);
    }

    let missing_inputs = report
        .inputs
        .iter()
        .filter(|input| input.status == "missing")
        .collect::<Vec<_>>();
    if !missing_inputs.is_empty() {
        let bindings = missing_inputs
            .iter()
            .map(|input| format!("{} -> {}", input.role, input.path.display()))
            .collect::<Vec<_>>()
            .join("; ");
        let mut steps = vec![FollowUpStep::manual(format!(
            "Rebuild or restore these already-bound artifacts: {bindings}."
        ))];
        if let Some(path) = report.run_spec.path.as_deref() {
            steps.push(FollowUpStep::command(
                format!(
                    "Only if an artifact path changed, recreate its binding in {}.",
                    path.display()
                ),
                context.inputs_init_help_action()?,
            ));
        }
        steps.push(FollowUpStep::command(
            "Recheck project readiness after restoring the inputs.",
            context.follow_up_action(["project", "status"], ProjectContextRequirement::RunSpec)?,
        ));
        return Ok(steps);
    }

    Ok(vec![
        FollowUpStep::command(
            "Inspect the prerequisite-ordered project file contract.",
            context.follow_up_action(
                ["project", "files", "--details"],
                ProjectContextRequirement::Target,
            )?,
        ),
        FollowUpStep::command(
            "Regenerate analysis evidence.",
            context
                .follow_up_action(["project", "analyze"], ProjectContextRequirement::Analysis)?,
        ),
        FollowUpStep::command(
            "Recheck project readiness.",
            context.follow_up_action(["project", "status"], ProjectContextRequirement::RunSpec)?,
        ),
    ])
}

pub(super) fn render(report: &DoctorReport) {
    output::render_report(report, || human(report));
}

fn human(report: &DoctorReport) {
    outputln!("{}", output::heading("Project doctor"));
    outputln!("Project:  {}", report.project.id);
    outputln!("Manifest: {}", report.project.path.display());
    outputln!(
        "Target:   {} ({})",
        report.target.id,
        report.target.path.display()
    );
    outputln!("Validation: deep configuration, input and reviewed-workspace inspection");
    outputln!("Freshness:  unknown; use project check for reproducibility");
    outputln!("Duration:   {} ms", report.duration_ms);

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

    if !report.next_steps.is_empty() {
        outputln!("\n{}", output::heading("Next"));
        for (index, step) in report.next_steps.iter().enumerate() {
            outputln!("{}. {}", index + 1, sanitize(&step.instruction));
            for command in &step.commands {
                outputln!("   {}", command.render_posix());
            }
        }
    }

    if output::details() {
        outputln!("\n{}", output::heading("Timings"));
        outputln!(
            "{}",
            table::render(
                ["Section", "Duration"],
                report.timings.iter().map(|timing| [
                    timing.section.to_owned(),
                    format!("{} ms", timing.duration_ms),
                ]),
            )
        );
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
