//! Project-owned publication of reviewed register artifacts.

use super::{Command, Result, registers};
use crate::MemoryMap;
use crate::project::{ProjectSpec, RegisterWorkspacePaths};

use super::project_pipeline::status::{
    PipelineSummary, StageOutcome, StageSuccess, execute, report,
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
    arguments: Vec<String>,
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
) -> Result<bool> {
    let options = parse_options(arguments)?;
    let paths = project
        .registers
        .as_ref()
        .ok_or("project publish requires a [registers] workspace")?;
    validate_output_paths(paths)?;
    let mut summary = PipelineSummary::default();

    let validation = execute("register-validation", StageSuccess::Verified, || {
        registers::run(
            Command::RegisterValidate,
            vec!["--deny-unreviewed".to_owned()],
            project,
            memory_map,
        )
    });
    report("register-validation", &validation, &mut summary);

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
                publication.write_or_check(options.check)
            }),
            Preparation::Failed(reason) => StageOutcome::Failed(reason),
            Preparation::NotConfigured(reason) => StageOutcome::NotConfigured(reason),
        };
        report(stage.name, &outcome, &mut summary);
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
                return Err(format!(
                    "project publication outputs {left_name} and {right_name} share {}",
                    left_path.display()
                )
                .into());
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
            return Err(format!(
                "project publication output {output_name} conflicts with {input_name} {}",
                output_path.display()
            )
            .into());
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
        report(name, &outcome, summary);
    }
}

fn finish(check: bool, summary: PipelineSummary) -> Result<bool> {
    println!(
        "PROJECT-PUBLICATION\tmode={}\tstatus={}\twritten={}\tverified={}\tfailed={}\tblocked={}\tnot-configured={}",
        if check { "check" } else { "write" },
        if summary.succeeded() { "ok" } else { "failed" },
        summary.written,
        summary.verified,
        summary.failed,
        summary.blocked,
        summary.not_configured,
    );
    Ok(summary.succeeded())
}

fn parse_options(arguments: Vec<String>) -> Result<Options> {
    let mut options = Options::default();
    for argument in arguments {
        match argument.as_str() {
            "--check" if !options.check => options.check = true,
            "--check" => return Err("duplicate --check".into()),
            _ => return Err(format!("unknown project publish option: {argument}").into()),
        }
    }
    Ok(options)
}

#[cfg(test)]
mod tests;
