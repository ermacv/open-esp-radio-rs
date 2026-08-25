//! User-facing immutable revision snapshot, diff and review-rebase workflow.

use std::path::PathBuf;

use serde::Serialize;

use crate::{
    Result,
    application::{generated_file, revision},
    cli::{
        RevisionDiffArgs, RevisionPrepareUpdateArgs, RevisionRebaseArgs, RevisionSnapshotArgs,
        output, resolver::RevisionWorkspaceCommand, table,
    },
};

#[derive(Serialize)]
struct SnapshotReport<'a> {
    schema_version: u32,
    command: &'static str,
    status: &'static str,
    name: &'a str,
    output: String,
    artifacts: usize,
    functions: usize,
    registers: usize,
    interfaces: usize,
    assertions: usize,
    vendor_bugs: usize,
    artifact_bindings_verified: usize,
}

pub(super) fn run(
    command: RevisionWorkspaceCommand,
    session: &crate::application::ProjectSession,
) -> Result<bool> {
    match command {
        RevisionWorkspaceCommand::Snapshot(arguments) => snapshot(arguments, session),
        RevisionWorkspaceCommand::PrepareUpdate(arguments) => prepare_update(arguments, session),
        RevisionWorkspaceCommand::Diff(arguments) => diff(arguments, session),
        RevisionWorkspaceCommand::Rebase(arguments) => rebase(arguments, session),
    }
}

fn snapshot(
    arguments: RevisionSnapshotArgs,
    session: &crate::application::ProjectSession,
) -> Result<bool> {
    // `persist_snapshot` resolves caller-supplied relative paths against the
    // project manifest. Keep the implicit path project-relative as well: a
    // manifest itself may be relative to the process cwd, and passing an
    // already manifest-prefixed relative path would apply that prefix twice.
    let path = arguments.output.unwrap_or_else(|| {
        PathBuf::from("revisions/snapshots").join(format!("{}.json.gz", arguments.name))
    });
    let snapshot = revision::snapshot(session, &arguments.name)?;
    let artifact_bindings_verified = revision::verify_snapshot_bindings(session, &snapshot)?;
    revision::persist_snapshot(&session.manifest, &snapshot, &path, arguments.check)?;
    let report = SnapshotReport {
        schema_version: revision::REVISION_SCHEMA,
        command: "revision snapshot",
        status: if arguments.check {
            "verified"
        } else {
            "written"
        },
        name: &snapshot.name,
        output: path.display().to_string(),
        artifacts: snapshot.artifacts.len(),
        functions: snapshot.functions.len(),
        registers: snapshot.registers.len(),
        interfaces: snapshot.interfaces.len(),
        assertions: snapshot.assertions.len(),
        vendor_bugs: snapshot.vendor_bugs.len(),
        artifact_bindings_verified,
    };
    output::render_report(&report, || {
        outputln!("{}", output::heading("Revision snapshot"));
        outputln!(
            "\n{}",
            output::success(format!(
                "{} — {} functions, {} registers, {} interfaces, {} reviewed records; {} live artifact identities verified",
                report.status.to_uppercase(),
                report.functions,
                report.registers,
                report.interfaces,
                report.assertions + report.vendor_bugs,
                report.artifact_bindings_verified,
            ))
        );
        outputln!("Output: {}", report.output);
    });
    Ok(true)
}

fn prepare_update(
    arguments: RevisionPrepareUpdateArgs,
    session: &crate::application::ProjectSession,
) -> Result<bool> {
    let report = revision::prepare_update(
        session,
        arguments.accept_current,
        arguments.check,
        arguments.migrate_legacy_scope.as_deref(),
    )?;
    output::render_report(&report, || {
        outputln!("{}", output::heading("Revision update preflight"));
        outputln!(
            "\n{}",
            output::success(format!(
                "{} — {} current artifact identities match the immutable snapshot",
                report.status.to_uppercase(),
                report.artifact_bindings_verified
            ))
        );
        outputln!("Ledger:   {}", report.ledger);
        outputln!("Baseline: {}", report.baseline);
        outputln!("Current:  {}", report.current);
        outputln!(
            "\nThe current blob may now be replaced. Keep the ledger and its snapshot, refresh analysis, then create a new named revision snapshot."
        );
    });
    Ok(true)
}

fn diff(arguments: RevisionDiffArgs, session: &crate::application::ProjectSession) -> Result<bool> {
    let from_path = revision::resolve_path(&session.manifest, &arguments.from)?;
    let to_path = revision::resolve_path(&session.manifest, &arguments.to)?;
    let from = revision::load(&from_path)?;
    let to = revision::load(&to_path)?;
    let report = revision::diff(&from, &to);
    if let Some(path) = arguments.output.as_deref() {
        generated_file::write_or_check_json(path, &report, arguments.check, "revision diff", true)?;
    }
    output::render_report(&report, || render_diff(&report));
    Ok(true)
}

fn rebase(
    arguments: RevisionRebaseArgs,
    session: &crate::application::ProjectSession,
) -> Result<bool> {
    let from_path = revision::resolve_path(&session.manifest, &arguments.from)?;
    let to_path = revision::resolve_path(&session.manifest, &arguments.to)?;
    let from = revision::load(&from_path)?;
    let to = revision::load(&to_path)?;
    let report = revision::rebase(&from, &to);
    if let Some(path) = arguments.output.as_deref() {
        generated_file::write_or_check_json(
            path,
            &report,
            arguments.check,
            "revision rebase plan",
            true,
        )?;
    }
    output::render_report(&report, || render_rebase(&report));
    Ok(true)
}

fn render_diff(report: &revision::RevisionDiffReport) {
    outputln!("{}", output::heading("Revision diff"));
    let summary = &report.summary;
    outputln!(
        "\n{}",
        if summary.modified + summary.removed + summary.split + summary.merged + summary.ambiguous
            == 0
        {
            output::success("NO REVIEW-BLOCKING DRIFT")
        } else {
            output::warning("REVIEW REQUIRED")
        }
    );
    outputln!(
        "\n{}",
        table::render(
            ["Classification", "Entities"],
            [
                ["Unchanged".to_owned(), summary.unchanged.to_string()],
                ["Moved".to_owned(), summary.moved.to_string()],
                ["Modified".to_owned(), summary.modified.to_string()],
                ["Added".to_owned(), summary.added.to_string()],
                ["Removed".to_owned(), summary.removed.to_string()],
                ["Split".to_owned(), summary.split.to_string()],
                ["Merged".to_owned(), summary.merged.to_string()],
                ["Ambiguous".to_owned(), summary.ambiguous.to_string()],
            ],
        )
    );
    if output::details() && !report.changes.is_empty() {
        outputln!("\n{}", output::heading("Changes"));
        outputln!(
            "{}",
            table::render(
                ["Domain", "Class", "Before", "After", "Confidence"],
                report.changes.iter().take(100).map(|change| [
                    change.domain.clone(),
                    format!("{:?}", change.classification).to_ascii_lowercase(),
                    change.before.join(", "),
                    change.after.join(", "),
                    change.confidence.clone(),
                ]),
            )
        );
        if report.changes.len() > 100 {
            outputln!(
                "… {} more change groups; use --format json or --output",
                report.changes.len() - 100
            );
        }
    }
}

fn render_rebase(report: &revision::RevisionRebaseReport) {
    outputln!("{}", output::heading("Revision rebase"));
    outputln!(
        "\n{}",
        if report.summary.review_required == 0 {
            output::success("REVIEWED PROGRESS IS CARRYABLE")
        } else {
            output::warning(format!(
                "{} RECORD(S) REQUIRE MANUAL REVIEW",
                report.summary.review_required
            ))
        }
    );
    outputln!(
        "\n{}",
        table::render(
            ["Status", "Records"],
            [
                [
                    "Already present".to_owned(),
                    report.summary.already_present.to_string(),
                ],
                [
                    "Carry exact".to_owned(),
                    report.summary.carry_exact.to_string()
                ],
                [
                    "Carry remapped".to_owned(),
                    report.summary.carry_remapped.to_string(),
                ],
                [
                    "Review required".to_owned(),
                    report.summary.review_required.to_string(),
                ],
            ],
        )
    );
    if output::details() && !report.records.is_empty() {
        outputln!("\n{}", output::heading("Reviewed records"));
        outputln!(
            "{}",
            table::render(
                ["ID", "Kind", "Status", "Proposed subject"],
                report.records.iter().map(|record| [
                    record.id.clone(),
                    record.kind.clone(),
                    format!("{:?}", record.status).to_ascii_lowercase(),
                    record
                        .proposed_subject
                        .clone()
                        .unwrap_or_else(|| "manual review".to_owned()),
                ]),
            )
        );
    }
}
