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
    bindings: usize,
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
        schema_version: revision::REVISION_SNAPSHOT_REPORT_SCHEMA,
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
        bindings: snapshot.bindings.len(),
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
                report.assertions + report.vendor_bugs + report.bindings,
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
    let report = revision::prepare_update(session, arguments.accept_current, arguments.check)?;
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
        outputln!("State:   {}", report.state);
        outputln!("Baseline: {}", report.baseline);
        outputln!("Current:  {}", report.current);
        outputln!(
            "\nThe current blob may now be replaced. Keep the state and its snapshot, refresh analysis, then create a new named revision snapshot."
        );
    });
    Ok(true)
}

fn diff(arguments: RevisionDiffArgs, session: &crate::application::ProjectSession) -> Result<bool> {
    let from = revision::load_operand(session, &arguments.from)?;
    let to = revision::load_operand(session, &arguments.to)?;
    revision::validate_operand_pair(&session.project.id, &from, &to)?;
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
    let from = revision::load_operand(session, &arguments.from)?;
    let to = revision::load_operand(session, &arguments.to)?;
    revision::validate_operand_pair(&session.project.id, &from, &to)?;
    let lineage = arguments
        .lineage
        .as_deref()
        .map(crate::symbol_lineage::load_rebase_evidence)
        .transpose()?;
    let report = revision::rebase(&from, &to, lineage.as_ref())?;
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
    let functions = &report.functions;
    outputln!("\n{}", output::heading("Function delta"));
    outputln!(
        "{}",
        table::render(
            ["Class", "Functions"],
            [
                ["Changed".to_owned(), functions.changed.len().to_string()],
                ["Added".to_owned(), functions.added.len().to_string()],
                ["Removed".to_owned(), functions.removed.len().to_string()],
                ["Remapped".to_owned(), functions.remapped.len().to_string()],
                [
                    "Uncertain".to_owned(),
                    functions.uncertain.len().to_string()
                ],
            ],
        )
    );
    if !report.invalidated_research.is_empty() {
        outputln!("\n{}", output::heading("Research invalidation"));
        outputln!(
            "{}",
            table::render(
                ["Area", "Subjects", "Reviewed facts"],
                report.invalidated_research.iter().map(|area| [
                    area.area.clone(),
                    area.subjects.len().to_string(),
                    area.reviewed_records.len().to_string(),
                ]),
            )
        );
        if output::details() {
            outputln!("\n{}", output::heading("Invalidation details"));
            outputln!(
                "{}",
                table::render(
                    ["Area", "Subjects", "Reviewed facts", "Reason"],
                    report.invalidated_research.iter().map(|area| [
                        area.area.clone(),
                        area.subjects.join(", "),
                        area.reviewed_records.join(", "),
                        area.reason.clone(),
                    ]),
                )
            );
        }
    }
    if output::details() {
        let rows = functions
            .changed
            .iter()
            .map(|id| ["changed".to_owned(), id.clone()])
            .chain(
                functions
                    .added
                    .iter()
                    .map(|id| ["added".to_owned(), id.clone()]),
            )
            .chain(
                functions
                    .removed
                    .iter()
                    .map(|id| ["removed".to_owned(), id.clone()]),
            )
            .chain(functions.remapped.iter().map(|remap| {
                [
                    "remapped".to_owned(),
                    format!("{} -> {}", remap.before, remap.after),
                ]
            }))
            .chain(
                functions
                    .uncertain
                    .iter()
                    .map(|id| ["uncertain".to_owned(), id.clone()]),
            )
            .collect::<Vec<_>>();
        if !rows.is_empty() {
            outputln!("\n{}", output::heading("Affected functions"));
            outputln!("{}", table::render(["Class", "Identity"], rows));
        }
    }
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
    if let Some(lineage) = &report.lineage {
        outputln!(
            "Lineage: {} mappings, report sha256:{} ({}@{} → {}@{})",
            lineage.mappings,
            lineage.report_sha256,
            lineage.source.source,
            lineage.source.sha256,
            lineage.target.source,
            lineage.target.sha256,
        );
    }
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
                ["ID", "Kind", "Status", "Proposed subject / occurrence"],
                report.records.iter().map(|record| [
                    record.id.clone(),
                    record.kind.clone(),
                    format!("{:?}", record.status).to_ascii_lowercase(),
                    [
                        record
                            .proposed_subject
                            .clone()
                            .unwrap_or_else(|| "manual review".to_owned()),
                        record
                            .proposed_occurrence
                            .as_ref()
                            .map(ToString::to_string)
                            .unwrap_or_default(),
                        record.proposed_locator.clone().unwrap_or_default(),
                    ]
                    .into_iter()
                    .filter(|value| !value.is_empty())
                    .collect::<Vec<_>>()
                    .join("\n"),
                ]),
            )
        );
    }
}
