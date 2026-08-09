//! Frontend-neutral orchestration of reviewed register publications.

use serde::Serialize;

use super::pipeline::{
    PipelineSummary, StageOutcome, StageSuccess, WorkflowMode, execute as execute_stage,
};
use crate::{
    MemoryMap, Result,
    project::RegisterWorkspacePaths,
    registers::{
        PreparedPublication, ProjectRegisterWorkspace, prepare_project_bindings,
        prepare_project_pac, prepare_project_svd, validate_pac_api, validate_register_evidence,
        validate_register_lints, validate_register_memory_map,
    },
};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProjectPublicationRequest {
    pub(crate) check: bool,
}

pub(crate) trait ProjectPublicationOperations {
    type Prepared;

    fn validate_registers(&mut self) -> Result<bool>;
    fn prepare_svd(&mut self) -> Result<Self::Prepared>;
    fn prepare_pac(&mut self) -> Result<Self::Prepared>;
    fn prepare_bindings(&mut self) -> Result<Self::Prepared>;
    fn publish(&mut self, publication: &Self::Prepared, check: bool) -> Result<bool>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ProjectPublicationReport {
    pub(crate) schema: u32,
    pub(crate) command: &'static str,
    pub(crate) mode: &'static str,
    pub(crate) status: &'static str,
    pub(crate) stages: Vec<super::pipeline::StageReport>,
    pub(crate) written: usize,
    pub(crate) verified: usize,
    pub(crate) failed: usize,
    pub(crate) blocked: usize,
    #[serde(rename = "not-configured")]
    pub(crate) not_configured: usize,
}

impl ProjectPublicationReport {
    pub(crate) const fn succeeded(&self) -> bool {
        self.failed == 0 && self.blocked == 0
    }
}

enum Preparation<T> {
    Ready(T),
    Failed(String),
    NotConfigured(String),
}

struct PublicationStage<T> {
    name: &'static str,
    preparation: Preparation<T>,
}

pub(crate) fn execute(
    paths: &RegisterWorkspacePaths,
    memory_map: Option<&MemoryMap>,
    request: ProjectPublicationRequest,
) -> Result<ProjectPublicationReport> {
    let mut operations = RegisterPublicationOperations { paths, memory_map };
    run_with_operations(paths, request, &mut operations)
}

fn run_with_operations<O: ProjectPublicationOperations>(
    paths: &RegisterWorkspacePaths,
    request: ProjectPublicationRequest,
    operations: &mut O,
) -> Result<ProjectPublicationReport> {
    validate_output_paths(paths)?;
    let mode = WorkflowMode::from_check(request.check);
    let mut summary = PipelineSummary::default();

    let validation = execute_stage("register-validation", StageSuccess::Verified, || {
        operations.validate_registers()
    });
    summary.record("register-validation", &validation);

    if validation.blocks_dependants() {
        report_blocked_publications(paths, &mut summary);
        return Ok(report(mode, summary));
    }

    let stages = [
        prepare_stage(
            "svd-publication",
            paths.svd_output.is_some(),
            "[registers.svd] is absent",
            || operations.prepare_svd(),
        ),
        prepare_stage(
            "pac-publication",
            paths.pac.is_some(),
            "[registers.pac] is absent",
            || operations.prepare_pac(),
        ),
        prepare_stage(
            "binding-publication",
            paths.bindings.is_some(),
            "[registers.bindings] is absent",
            || operations.prepare_bindings(),
        ),
    ];
    let preflight_failed = stages
        .iter()
        .any(|stage| matches!(stage.preparation, Preparation::Failed(_)));

    for stage in stages {
        let outcome = match stage.preparation {
            Preparation::Ready(_) if preflight_failed => {
                StageOutcome::Blocked("publication preflight did not complete".to_owned())
            }
            Preparation::Ready(publication) => {
                execute_stage(stage.name, mode.generated_success(), || {
                    operations.publish(&publication, mode.is_check())
                })
            }
            Preparation::Failed(reason) => StageOutcome::Failed(reason),
            Preparation::NotConfigured(reason) => StageOutcome::NotConfigured(reason),
        };
        summary.record(stage.name, &outcome);
    }

    Ok(report(mode, summary))
}

struct RegisterPublicationOperations<'a> {
    paths: &'a RegisterWorkspacePaths,
    memory_map: Option<&'a MemoryMap>,
}

impl ProjectPublicationOperations for RegisterPublicationOperations<'_> {
    type Prepared = PreparedPublication;

    fn validate_registers(&mut self) -> Result<bool> {
        let workspace = ProjectRegisterWorkspace::load(self.paths)?;
        let summary = workspace.summary()?;
        validate_pac_api(self.paths)?;
        validate_register_lints(self.paths)?;
        validate_register_memory_map(self.paths, self.memory_map)?;
        validate_register_evidence(self.paths, self.memory_map)?;
        Ok(summary.unreviewed == 0)
    }

    fn prepare_svd(&mut self) -> Result<Self::Prepared> {
        prepare_project_svd(self.paths)
    }

    fn prepare_pac(&mut self) -> Result<Self::Prepared> {
        prepare_project_pac(self.paths)
    }

    fn prepare_bindings(&mut self) -> Result<Self::Prepared> {
        prepare_project_bindings(self.paths)
    }

    fn publish(&mut self, publication: &Self::Prepared, check: bool) -> Result<bool> {
        super::generated_file::write_or_check(
            publication.output(),
            publication.contents(),
            check,
            publication.kind(),
        )?;
        Ok(true)
    }
}

fn report(mode: WorkflowMode, summary: PipelineSummary) -> ProjectPublicationReport {
    ProjectPublicationReport {
        schema: 1,
        command: "project publish",
        mode: mode.label(),
        status: if summary.succeeded() { "ok" } else { "failed" },
        stages: summary.stages().to_vec(),
        written: summary.written,
        verified: summary.verified,
        failed: summary.failed,
        blocked: summary.blocked,
        not_configured: summary.not_configured,
    }
}

fn prepare_stage<T>(
    name: &'static str,
    configured: bool,
    absent_reason: &'static str,
    prepare: impl FnOnce() -> Result<T>,
) -> PublicationStage<T> {
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
        summary.record(name, &outcome);
    }
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
