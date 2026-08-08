//! Project-owned publication of reviewed register artifacts.

use super::{Result, registers};
use crate::cli::resolver::RegisterWorkspaceCommand;
use serde::Serialize;

use crate::MemoryMap;
use crate::cli::{CheckArgs, ValidationArgs};
use crate::project::{ProjectSpec, RegisterWorkspacePaths};

use super::project_pipeline::status::{
    PipelineSummary, StageOutcome, StageReport, StageSuccess, execute, print_stage_tsv, record,
};

#[derive(Debug, Default, Eq, PartialEq)]
struct Options {
    check: bool,
}

enum Preparation {
    Ready(registers::PreparedPublication),
    Failed(String),
    NotConfigured(String),
}

struct PublicationStage {
    name: &'static str,
    preparation: Preparation,
}

pub(super) fn run(
    arguments: CheckArgs,
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
) -> Result<bool> {
    let options = Options {
        check: arguments.check,
    };
    let paths = project
        .registers
        .as_ref()
        .ok_or("project publish requires a [registers] workspace")
        .map_err(crate::Error::invalid)?;
    validate_output_paths(paths)?;
    let mut summary = PipelineSummary::default();

    let validation = execute("register-validation", StageSuccess::Verified, || {
        registers::run(
            RegisterWorkspaceCommand::Validate(ValidationArgs {
                deny_unreviewed: true,
            }),
            project,
            memory_map,
        )
    });
    record("register-validation", &validation, &mut summary);

    if validation.blocks_dependants() {
        report_blocked_publications(paths, &mut summary);
        return finish(options.check, summary);
    }

    let stages = prepare_publications(paths);
    let preflight_failed = stages
        .iter()
        .any(|stage| matches!(stage.preparation, Preparation::Failed(_)));
    let success = if options.check {
        StageSuccess::Verified
    } else {
        StageSuccess::Written
    };

    for stage in stages {
        let outcome = match stage.preparation {
            Preparation::Ready(_) if preflight_failed => {
                StageOutcome::Blocked("publication preflight did not complete".to_owned())
            }
            Preparation::Ready(publication) => execute(stage.name, success, || {
                registers::write_prepared_publication(&publication, options.check)
            }),
            Preparation::Failed(reason) => StageOutcome::Failed(reason),
            Preparation::NotConfigured(reason) => StageOutcome::NotConfigured(reason),
        };
        record(stage.name, &outcome, &mut summary);
    }

    finish(options.check, summary)
}

fn prepare_publications(paths: &RegisterWorkspacePaths) -> [PublicationStage; 3] {
    [
        prepare_stage(
            "svd-publication",
            paths.svd_output.is_some(),
            "[registers.svd] is absent",
            || registers::prepare_project_svd(paths),
        ),
        prepare_stage(
            "pac-publication",
            paths.pac.is_some(),
            "[registers.pac] is absent",
            || registers::prepare_project_pac(paths),
        ),
        prepare_stage(
            "binding-publication",
            paths.bindings.is_some(),
            "[registers.bindings] is absent",
            || registers::prepare_project_bindings(paths),
        ),
    ]
}

fn validate_output_paths(paths: &RegisterWorkspacePaths) -> Result<()> {
    let mut outputs = Vec::new();
    if let Some(path) = paths.svd_output.as_deref() {
        outputs.push(("svd-publication", path));
    }
    if let Some(pac) = paths.pac.as_ref() {
        outputs.push(("pac-publication", pac.output.as_path()));
    }
    if let Some(bindings) = paths.bindings.as_ref() {
        outputs.push(("binding-publication", bindings.output.as_path()));
    }
    for (index, (left_name, left_path)) in outputs.iter().enumerate() {
        for (right_name, right_path) in outputs.iter().skip(index + 1) {
            if left_path == right_path {
                return Err(crate::Error::invalid(format!(
                    "project publication outputs {left_name} and {right_name} share {}",
                    left_path.display()
                )));
            }
        }
    }

    let mut inputs = vec![
        ("register facts", paths.facts.as_path()),
        ("register model", paths.model.as_path()),
    ];
    inputs.extend(
        paths
            .review_output
            .as_deref()
            .map(|path| ("register review", path)),
    );
    inputs.extend(
        paths
            .review_ir_reports
            .iter()
            .map(|path| ("linked-IR review input", path.as_path())),
    );
    inputs.extend(paths.api_pack.as_deref().map(|path| ("PAC API pack", path)));
    inputs.extend(
        paths
            .lint_pack
            .as_deref()
            .map(|path| ("register lint pack", path)),
    );
    inputs.extend(
        paths
            .evidence_catalogs
            .iter()
            .map(|path| ("register evidence catalog", path.as_path())),
    );
    for (output_name, output_path) in outputs {
        if let Some((input_name, _)) = inputs
            .iter()
            .find(|(_, input_path)| *input_path == output_path)
        {
            return Err(crate::Error::invalid(format!(
                "project publication output {output_name} conflicts with {input_name} {}",
                output_path.display()
            )));
        }
    }
    Ok(())
}

fn prepare_stage(
    name: &'static str,
    configured: bool,
    absent_reason: &'static str,
    prepare: impl FnOnce() -> Result<registers::PreparedPublication>,
) -> PublicationStage {
    let preparation = if configured {
        match prepare() {
            Ok(publication) => Preparation::Ready(publication),
            Err(error) => Preparation::Failed(error.to_string()),
        }
    } else {
        Preparation::NotConfigured(absent_reason.to_owned())
    };
    PublicationStage { name, preparation }
}

fn report_blocked_publications(paths: &RegisterWorkspacePaths, summary: &mut PipelineSummary) {
    for (name, configured, absent_reason) in [
        (
            "svd-publication",
            paths.svd_output.is_some(),
            "[registers.svd] is absent",
        ),
        (
            "pac-publication",
            paths.pac.is_some(),
            "[registers.pac] is absent",
        ),
        (
            "binding-publication",
            paths.bindings.is_some(),
            "[registers.bindings] is absent",
        ),
    ] {
        let outcome = if configured {
            StageOutcome::Blocked("register-validation did not complete".to_owned())
        } else {
            StageOutcome::NotConfigured(absent_reason.to_owned())
        };
        record(name, &outcome, summary);
    }
}

#[derive(Serialize)]
struct PublicationDocument<'a> {
    schema: u32,
    command: &'static str,
    mode: &'static str,
    status: &'static str,
    stages: &'a [StageReport],
    written: usize,
    verified: usize,
    failed: usize,
    blocked: usize,
    #[serde(rename = "not-configured")]
    not_configured: usize,
}

fn finish(check: bool, summary: PipelineSummary) -> Result<bool> {
    let document = PublicationDocument {
        schema: 1,
        command: "project publish",
        mode: if check { "check" } else { "write" },
        status: if summary.succeeded() { "ok" } else { "failed" },
        stages: summary.stages(),
        written: summary.written,
        verified: summary.verified,
        failed: summary.failed,
        blocked: summary.blocked,
        not_configured: summary.not_configured,
    };
    crate::cli::output::render_report(
        &document,
        || print_human(&document),
        || print_tsv(&document),
    );
    Ok(summary.succeeded())
}

fn print_human(document: &PublicationDocument<'_>) {
    outputln!(
        "Project publication: {} ({})",
        document.status,
        document.mode
    );
    for stage in document.stages {
        outputln!(
            "  {:<24} {:<14} {}",
            stage.name,
            stage.status,
            stage.reason.as_deref().unwrap_or("")
        );
    }
    outputln!(
        "  written={} verified={} failed={} blocked={} not-configured={}",
        document.written,
        document.verified,
        document.failed,
        document.blocked,
        document.not_configured
    );
}

fn print_tsv(document: &PublicationDocument<'_>) {
    for stage in document.stages {
        print_stage_tsv(stage);
    }
    outputln!(
        "PROJECT-PUBLICATION\tmode={}\tstatus={}\twritten={}\tverified={}\tfailed={}\tblocked={}\tnot-configured={}",
        document.mode,
        document.status,
        document.written,
        document.verified,
        document.failed,
        document.blocked,
        document.not_configured,
    );
}

#[cfg(test)]
mod tests;
