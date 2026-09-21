//! Project-check argument and output adapters.
use super::Result;
use crate::{
    application::{
        ProjectSession,
        project_check::{ProjectCheckReport, ProjectCheckRequest},
    },
    cli::{ProjectCheckArgs, ProjectVerifyArgs},
};
pub(super) fn run(arguments: ProjectCheckArgs, session: &ProjectSession) -> Result<bool> {
    let report = crate::application::project_check::execute(
        ProjectCheckRequest {
            deny_unreviewed: arguments.deny_unreviewed,
            jobs: usize::from(arguments.jobs),
        },
        session,
        |session| {
            super::project_verification::execute(
                ProjectVerifyArgs {
                    check: true,
                    ..Default::default()
                },
                &session.manifest,
                &session.project,
                session.run_spec.as_ref(),
                &session.mmio,
                &session.target,
            )
        },
    )?;
    crate::cli::output::render_report(&report, || render_human(&report));
    Ok(report.passed)
}

fn render_human(report: &ProjectCheckReport) {
    use crate::cli::{output, table};

    outputln!("{}", output::heading("Project check"));
    outputln!("Project: {}", report.project);
    let outcome = if report.passed {
        output::success("PASS — analysis, verification policy and publication reproduce")
    } else {
        output::failure("FAIL — one or more project gates did not reproduce")
    };
    outputln!("\n{outcome}");

    let issues = report
        .stages
        .iter()
        .flat_map(|stage| &stage.issues)
        .collect::<Vec<_>>();
    if !issues.is_empty() {
        outputln!("\n{}", output::heading("Problems"));
        for (index, issue) in issues.iter().enumerate() {
            outputln!(
                "{}. {} [{}]: {}",
                index + 1,
                issue.component,
                issue.status,
                issue.reason
            );
        }
    }

    let steps = report
        .stages
        .iter()
        .flat_map(|stage| &stage.next_steps)
        .collect::<Vec<_>>();
    if !steps.is_empty() {
        outputln!("\n{}", output::heading("Next"));
        for (index, step) in steps.iter().enumerate() {
            outputln!("{}. {}", index + 1, step.instruction);
            for command in &step.commands {
                outputln!("   {}", command.render_posix());
            }
        }
    }

    outputln!("\n{}", output::heading("Gates"));
    outputln!(
        "{}",
        table::render(
            ["Gate", "Status", "Summary"],
            report.stages.iter().map(|stage| [
                stage.name.to_owned(),
                stage.status.to_owned(),
                stage.summary.clone(),
            ])
        )
    );
}
