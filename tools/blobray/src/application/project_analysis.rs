//! Frontend-neutral orchestration of project-owned analysis and review stages.

mod cache;
mod operations;
mod plan;

pub(crate) use operations::*;
pub use plan::*;

use std::path::Path;

use serde::Serialize;

use super::pipeline::{
    PipelineSummary, StageExecution, StageRun, StageSuccess, WorkflowMode, execute,
};
use crate::{Result, project::ProjectSpec};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProjectAnalysisRequest {
    pub check: bool,
    pub deny_unreviewed: bool,
    pub jobs: usize,
}

impl ProjectAnalysisRequest {
    pub const MIN_JOBS: usize = 1;
    pub const MAX_JOBS: usize = 8;

    pub(crate) fn validate(self) -> Result<Self> {
        if !(Self::MIN_JOBS..=Self::MAX_JOBS).contains(&self.jobs) {
            return Err(crate::Error::invalid(format!(
                "project analysis jobs must be in {}..={}, got {}",
                Self::MIN_JOBS,
                Self::MAX_JOBS,
                self.jobs
            )));
        }
        Ok(self)
    }
}

impl Default for ProjectAnalysisRequest {
    fn default() -> Self {
        Self {
            check: false,
            deny_unreviewed: false,
            jobs: 1,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProjectAnalysisInputs {
    pub(crate) run_spec: bool,
    pub(crate) memory_map: bool,
    pub(crate) event_replays: bool,
    pub(crate) event_replays_require_interfaces: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ProjectAnalysisStatus {
    #[serde(rename = "ok")]
    Complete,
    NothingConfigured,
    Failed,
}

/// Operations required by the project analysis coordinator.
///
/// The coordinator owns ordering and dependency policy. Implementations own
/// the concrete analysis engines and artifact publication boundaries.
pub(crate) trait ProjectAnalysisOperations {
    /// Verify that every caller-owned, local configuration and reviewed input
    /// still belongs to the generation captured before orchestration began.
    fn validate_pipeline_inputs(&mut self) -> Result<()> {
        Ok(())
    }

    fn symbol_inventory(&mut self, check: bool) -> Result<StageRun>;
    fn discover_mmio(&mut self, check: bool, jobs: usize) -> Result<StageRun>;
    fn discover_interfaces(&mut self, check: bool) -> Result<StageRun>;
    fn build_linked_ir(&mut self, check: bool, jobs: usize) -> Result<StageRun>;
    fn build_event_replays(&mut self, check: bool) -> Result<StageRun>;
    fn build_review_scopes(&mut self, check: bool) -> Result<StageRun>;
    fn build_navigation(&mut self, check: bool) -> Result<StageRun>;
    fn validate_code(&mut self, deny_unreviewed: bool) -> Result<StageRun>;
    fn review_code(&mut self, check: bool) -> Result<StageRun>;
    fn validate_registers(&mut self, deny_unreviewed: bool) -> Result<StageRun>;
    fn review_registers(&mut self, check: bool) -> Result<StageRun>;
    fn validate_functions(&mut self, deny_unreviewed: bool) -> Result<StageRun>;
    fn review_functions(&mut self, check: bool) -> Result<StageRun>;
    fn validate_interfaces(&mut self, deny_unreviewed: bool) -> Result<StageRun>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ProjectAnalysisReport {
    pub schema: u32,
    pub command: &'static str,
    pub mode: &'static str,
    pub status: ProjectAnalysisStatus,
    pub stages: Vec<super::pipeline::StageReport>,
    pub written: usize,
    pub restored: usize,
    pub verified: usize,
    #[serde(rename = "up-to-date")]
    pub current: usize,
    pub failed: usize,
    pub blocked: usize,
    #[serde(rename = "not-configured")]
    pub not_configured: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl ProjectAnalysisReport {
    pub const fn succeeded(&self) -> bool {
        matches!(self.status, ProjectAnalysisStatus::Complete)
    }
}

pub(crate) fn run(
    project: &ProjectSpec,
    request: ProjectAnalysisRequest,
    inputs: ProjectAnalysisInputs,
    operations: &mut impl ProjectAnalysisOperations,
) -> ProjectAnalysisReport {
    let mode = WorkflowMode::from_check(request.check);
    let generated = mode.generated_success();
    let mut summary = PipelineSummary::default();

    let symbols = match project.symbol_inventory.as_ref() {
        None => StageExecution::not_configured("[analysis.symbols] is absent"),
        Some(_) if !inputs.run_spec => StageExecution::blocked("run-spec is not configured"),
        Some(_) => execute("symbol-inventory", generated, || {
            operations.validate_pipeline_inputs()?;
            operations.symbol_inventory(mode.is_check())
        }),
    };
    summary.record("symbol-inventory", &symbols);

    let mmio = match project.registers.as_ref() {
        None => StageExecution::not_configured("[registers] is absent"),
        Some(_) if !inputs.run_spec => StageExecution::blocked("run-spec is not configured"),
        Some(_) if !inputs.memory_map => StageExecution::blocked("memory-map is not configured"),
        Some(_) if project.code.is_some() && symbols.blocks_dependants() => {
            StageExecution::blocked("symbol-inventory did not complete")
        }
        Some(_) => execute("mmio-discovery", generated, || {
            operations.validate_pipeline_inputs()?;
            operations.discover_mmio(mode.is_check(), request.jobs)
        }),
    };
    summary.record("mmio-discovery", &mmio);

    let interfaces = match project.interfaces.as_ref() {
        None => StageExecution::not_configured("[interfaces] is absent"),
        Some(_) if !inputs.run_spec => StageExecution::blocked("run-spec is not configured"),
        Some(_) if project.code.is_some() && symbols.blocks_dependants() => {
            StageExecution::blocked("symbol-inventory did not complete")
        }
        Some(_) => execute("interface-discovery", generated, || {
            operations.validate_pipeline_inputs()?;
            operations.discover_interfaces(mode.is_check())
        }),
    };
    summary.record("interface-discovery", &interfaces);

    let ir = if project.ir_profiles.is_empty() {
        StageExecution::not_configured("[[analysis.ir]] is absent")
    } else if !inputs.run_spec {
        StageExecution::blocked("run-spec is not configured")
    } else if project.code.is_some() && symbols.blocks_dependants() {
        StageExecution::blocked("symbol-inventory did not complete")
    } else if linked_ir_uses_reviewed_interfaces(project) && interfaces.blocks_dependants() {
        StageExecution::blocked("interface-discovery did not complete")
    } else {
        execute("linked-ir", generated, || {
            operations.validate_pipeline_inputs()?;
            operations.build_linked_ir(mode.is_check(), request.jobs)
        })
    };
    summary.record("linked-ir", &ir);

    let event_replays = if !inputs.event_replays {
        StageExecution::not_configured("no reviewed event replay is configured")
    } else if !inputs.run_spec {
        StageExecution::blocked("run-spec is not configured")
    } else if inputs.event_replays_require_interfaces && interfaces.blocks_dependants() {
        StageExecution::blocked("interface-discovery did not complete")
    } else {
        execute("event-replays", generated, || {
            operations.validate_pipeline_inputs()?;
            operations.build_event_replays(mode.is_check())
        })
    };
    summary.record("event-replays", &event_replays);

    let review_scopes = match project.review.as_ref() {
        None => StageExecution::not_configured("[review] is absent"),
        Some(_) if ir.blocks_dependants() => StageExecution::blocked("linked-ir did not complete"),
        Some(_) if project.registers.is_some() && mmio.blocks_dependants() => {
            StageExecution::blocked("mmio-discovery did not complete")
        }
        Some(_) => execute("review-scopes", generated, || {
            operations.validate_pipeline_inputs()?;
            operations.build_review_scopes(mode.is_check())
        }),
    };
    summary.record("review-scopes", &review_scopes);

    let navigation = match project.navigation_index.as_ref() {
        None => StageExecution::not_configured("[analysis.navigation] is absent"),
        Some(_) if symbols.blocks_dependants() => {
            StageExecution::blocked("symbol-inventory did not complete")
        }
        Some(_) if !project.ir_profiles.is_empty() && ir.blocks_dependants() => {
            StageExecution::blocked("linked-ir did not complete")
        }
        Some(_) if project.interfaces.is_some() && interfaces.blocks_dependants() => {
            StageExecution::blocked("interface-discovery did not complete")
        }
        Some(_) => execute("navigation-index", generated, || {
            operations.validate_pipeline_inputs()?;
            operations.build_navigation(mode.is_check())
        }),
    };
    summary.record("navigation-index", &navigation);

    let code_validation = match project.code.as_ref() {
        None => StageExecution::not_configured("[code] is absent"),
        Some(_) if symbols.blocks_dependants() => {
            StageExecution::blocked("symbol-inventory did not complete")
        }
        Some(_) => execute("code-boundary-validation", StageSuccess::Verified, || {
            operations.validate_pipeline_inputs()?;
            operations.validate_code(request.deny_unreviewed)
        }),
    };
    summary.record("code-boundary-validation", &code_validation);

    let code_review = match project.code.as_ref() {
        None => StageExecution::not_configured("[code] is absent"),
        Some(paths) if paths.review_output.is_none() => {
            StageExecution::not_configured("[code.review] is absent")
        }
        Some(_) if symbols.blocks_dependants() => {
            StageExecution::blocked("symbol-inventory did not complete")
        }
        Some(_) => execute("code-boundary-review", generated, || {
            operations.validate_pipeline_inputs()?;
            operations.review_code(mode.is_check())
        }),
    };
    summary.record("code-boundary-review", &code_review);

    let register_validation = match project.registers.as_ref() {
        None => StageExecution::not_configured("[registers] is absent"),
        Some(_) if mmio.blocks_dependants() => {
            StageExecution::blocked("mmio-discovery did not complete")
        }
        Some(_) => execute("register-validation", StageSuccess::Verified, || {
            operations.validate_pipeline_inputs()?;
            operations.validate_registers(request.deny_unreviewed)
        }),
    };
    summary.record("register-validation", &register_validation);

    let register_review = match project.registers.as_ref() {
        None => StageExecution::not_configured("[registers] is absent"),
        Some(paths) if paths.review_output.is_none() => {
            StageExecution::not_configured("[registers.review] is absent")
        }
        Some(_) if mmio.blocks_dependants() => {
            StageExecution::blocked("mmio-discovery did not complete")
        }
        Some(paths)
            if review_depends_on_project_ir(project, &paths.review_ir_reports)
                && ir.blocks_dependants() =>
        {
            StageExecution::blocked("linked-ir did not complete")
        }
        Some(_) => execute("register-review", generated, || {
            operations.validate_pipeline_inputs()?;
            operations.review_registers(mode.is_check())
        }),
    };
    summary.record("register-review", &register_review);

    let function_validation = match project.functions.as_ref() {
        None => StageExecution::not_configured("[functions] is absent"),
        Some(_) if ir.blocks_dependants() => StageExecution::blocked("linked-ir did not complete"),
        Some(_) => execute("function-validation", StageSuccess::Verified, || {
            operations.validate_pipeline_inputs()?;
            operations.validate_functions(request.deny_unreviewed)
        }),
    };
    summary.record("function-validation", &function_validation);

    let function_review = match project.functions.as_ref() {
        None => StageExecution::not_configured("[functions] is absent"),
        Some(paths) if paths.review_output.is_none() => {
            StageExecution::not_configured("[functions.review] is absent")
        }
        Some(_) if ir.blocks_dependants() => StageExecution::blocked("linked-ir did not complete"),
        Some(_)
            if project
                .interfaces
                .as_ref()
                .and_then(|paths| paths.pack.as_deref())
                .is_some_and(Path::is_file)
                && interfaces.blocks_dependants() =>
        {
            StageExecution::blocked("interface-discovery did not complete")
        }
        Some(_) => execute("function-review", generated, || {
            operations.validate_pipeline_inputs()?;
            operations.review_functions(mode.is_check())
        }),
    };
    summary.record("function-review", &function_review);

    let interface_validation = match project.interfaces.as_ref() {
        None => StageExecution::not_configured("[interfaces] is absent"),
        Some(paths) if paths.pack.is_none() => {
            StageExecution::not_configured("[interfaces].pack is absent")
        }
        Some(_) if interfaces.blocks_dependants() => {
            StageExecution::blocked("interface-discovery did not complete")
        }
        Some(_) => execute("interface-validation", StageSuccess::Verified, || {
            operations.validate_pipeline_inputs()?;
            operations.validate_interfaces(request.deny_unreviewed)
        }),
    };
    summary.record("interface-validation", &interface_validation);

    if summary.succeeded()
        && let Err(error) = operations.validate_pipeline_inputs()
    {
        summary.record(
            "input-consistency",
            &StageExecution::failed(format!(
                "project inputs changed before analysis completion: {error}"
            )),
        );
    }

    let status = if !summary.succeeded() {
        ProjectAnalysisStatus::Failed
    } else if summary.written + summary.restored + summary.verified + summary.current == 0 {
        ProjectAnalysisStatus::NothingConfigured
    } else {
        ProjectAnalysisStatus::Complete
    };
    ProjectAnalysisReport {
        schema: 5,
        command: "project analyze",
        mode: mode.label(),
        status,
        stages: summary.stages().to_vec(),
        written: summary.written,
        restored: summary.restored,
        verified: summary.verified,
        current: summary.current,
        failed: summary.failed,
        blocked: summary.blocked,
        not_configured: summary.not_configured,
        duration_ms: summary.duration_ms,
    }
}

fn review_depends_on_project_ir(project: &ProjectSpec, reports: &[std::path::PathBuf]) -> bool {
    project
        .ir_profiles
        .iter()
        .any(|profile| reports.iter().any(|report| report == &profile.output))
}

pub(super) fn linked_ir_uses_reviewed_interfaces(project: &ProjectSpec) -> bool {
    project
        .interfaces
        .as_ref()
        .is_some_and(|interfaces| interfaces.pack.is_some())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_analysis::SymbolInventorySpec;

    #[derive(Default)]
    struct FakeOperations {
        calls: Vec<&'static str>,
        current: bool,
        fail: Option<&'static str>,
        pipeline_inputs: Option<super::cache::PipelineInputObservation>,
        rebind_after: Option<&'static str>,
        rebind: Option<InputRebind>,
    }

    struct InputRebind {
        input: std::path::PathBuf,
        saved: std::path::PathBuf,
        replacement: std::path::PathBuf,
    }

    impl FakeOperations {
        fn called(&mut self, name: &'static str) -> Result<StageRun> {
            self.calls.push(name);
            if self.fail == Some(name) {
                return Err(crate::Error::invalid(format!("{name} failed")));
            }
            if self.rebind_after == Some(name) {
                self.rebind_after = None;
                let rebind = self.rebind.as_ref().expect("rebind fixture is configured");
                std::fs::rename(&rebind.input, &rebind.saved)?;
                std::fs::rename(&rebind.replacement, &rebind.input)?;
                std::fs::rename(&rebind.input, &rebind.replacement)?;
                std::fs::rename(&rebind.saved, &rebind.input)?;
            }
            Ok(if self.current {
                StageRun::Current
            } else {
                StageRun::Executed
            })
        }
    }

    impl ProjectAnalysisOperations for FakeOperations {
        fn validate_pipeline_inputs(&mut self) -> Result<()> {
            if let Some(inputs) = self.pipeline_inputs.as_mut() {
                inputs.validate()?;
            }
            Ok(())
        }

        fn symbol_inventory(&mut self, _: bool) -> Result<StageRun> {
            self.called("symbols")
        }

        fn discover_mmio(&mut self, _: bool, _: usize) -> Result<StageRun> {
            self.called("mmio")
        }

        fn discover_interfaces(&mut self, _: bool) -> Result<StageRun> {
            self.called("interfaces")
        }

        fn build_linked_ir(&mut self, _: bool, _: usize) -> Result<StageRun> {
            self.called("ir")
        }

        fn build_event_replays(&mut self, _: bool) -> Result<StageRun> {
            self.called("event-replays")
        }

        fn build_review_scopes(&mut self, _: bool) -> Result<StageRun> {
            self.called("review-scopes")
        }

        fn build_navigation(&mut self, _: bool) -> Result<StageRun> {
            self.called("navigation")
        }

        fn validate_code(&mut self, _: bool) -> Result<StageRun> {
            self.called("code-validation")
        }

        fn review_code(&mut self, _: bool) -> Result<StageRun> {
            self.called("code-review")
        }

        fn validate_registers(&mut self, _: bool) -> Result<StageRun> {
            self.called("register-validation")
        }

        fn review_registers(&mut self, _: bool) -> Result<StageRun> {
            self.called("register-review")
        }

        fn validate_functions(&mut self, _: bool) -> Result<StageRun> {
            self.called("function-validation")
        }

        fn review_functions(&mut self, _: bool) -> Result<StageRun> {
            self.called("function-review")
        }

        fn validate_interfaces(&mut self, _: bool) -> Result<StageRun> {
            self.called("interface-validation")
        }
    }

    fn empty_project() -> ProjectSpec {
        ProjectSpec {
            id: "fixture".to_owned(),
            target_spec: "target.toml".into(),
            ecosystem_packs: Vec::new(),
            chip_pack: None,
            run_spec: None,
            memory_map: None,
            svd_paths: Vec::new(),
            symbol_inventory: None,
            navigation_index: None,
            code: None,
            ir_profiles: Vec::new(),
            registers: None,
            interfaces: None,
            functions: None,
            review: None,
            verification: None,
        }
    }

    #[test]
    fn public_request_bounds_are_shared_with_all_frontends() {
        for jobs in [0, ProjectAnalysisRequest::MAX_JOBS + 1] {
            let error = ProjectAnalysisRequest {
                jobs,
                ..ProjectAnalysisRequest::default()
            }
            .validate()
            .unwrap_err();
            assert!(error.to_string().contains("jobs must be in 1..=8"));
        }
        assert!(
            ProjectAnalysisRequest {
                jobs: ProjectAnalysisRequest::MIN_JOBS,
                ..ProjectAnalysisRequest::default()
            }
            .validate()
            .is_ok()
        );
        assert!(
            ProjectAnalysisRequest {
                jobs: ProjectAnalysisRequest::MAX_JOBS,
                ..ProjectAnalysisRequest::default()
            }
            .validate()
            .is_ok()
        );
    }

    #[test]
    fn optional_absence_is_a_typed_non_successful_noop() {
        let mut operations = FakeOperations::default();
        let report = run(
            &empty_project(),
            ProjectAnalysisRequest::default(),
            ProjectAnalysisInputs::default(),
            &mut operations,
        );
        assert!(operations.calls.is_empty());
        assert_eq!(report.status, ProjectAnalysisStatus::NothingConfigured);
        assert!(!report.succeeded());
        assert_eq!(report.not_configured, 14);
        assert_eq!(report.duration_ms, None);
        assert!(
            report
                .stages
                .iter()
                .all(|stage| stage.duration_ms.is_none())
        );
        assert_eq!(
            serde_json::to_value(&report).unwrap()["status"],
            "nothing-configured"
        );
    }

    #[test]
    fn missing_caller_inputs_block_a_configured_root_without_executing_it() {
        let mut project = empty_project();
        project.symbol_inventory = Some(SymbolInventorySpec {
            output: "symbols.json".into(),
        });
        let mut operations = FakeOperations::default();
        let report = run(
            &project,
            ProjectAnalysisRequest::default(),
            ProjectAnalysisInputs::default(),
            &mut operations,
        );
        assert!(operations.calls.is_empty());
        assert!(!report.succeeded());
        assert_eq!(report.blocked, 1);
        assert_eq!(report.stages[0].status, "blocked");
        assert_eq!(report.stages[0].duration_ms, None);
    }

    #[test]
    fn current_stage_is_successful_and_reported_separately_from_a_write() {
        let mut project = empty_project();
        project.symbol_inventory = Some(SymbolInventorySpec {
            output: "symbols.json".into(),
        });
        let mut operations = FakeOperations {
            current: true,
            ..FakeOperations::default()
        };
        let report = run(
            &project,
            ProjectAnalysisRequest::default(),
            ProjectAnalysisInputs {
                run_spec: true,
                memory_map: false,
                event_replays: false,
                event_replays_require_interfaces: false,
            },
            &mut operations,
        );
        assert!(report.succeeded());
        assert_eq!(report.status, ProjectAnalysisStatus::Complete);
        assert_eq!(report.written, 0);
        assert_eq!(report.current, 1);
        assert_eq!(report.stages[0].status, "up-to-date");
        assert!(report.stages[0].duration_ms.is_some());
        assert_eq!(
            report.duration_ms,
            Some(
                report
                    .stages
                    .iter()
                    .filter_map(|stage| stage.duration_ms)
                    .sum::<u64>()
            )
        );
    }

    #[test]
    fn linked_ir_is_blocked_when_its_reviewed_interface_predecessor_fails() {
        let mut project = empty_project();
        project.interfaces = Some(crate::project::InterfaceWorkspacePaths {
            facts: "interfaces.json".into(),
            pack: Some("interfaces.toml".into()),
            semantic_catalogs: Vec::new(),
        });
        project
            .ir_profiles
            .push(crate::project_ir::ProjectIrProfile {
                id: "fixture".to_owned(),
                sources: vec!["fixture".to_owned()],
                roots: crate::project_ir::ProjectIrRoots::All,
                include_reachable: true,
                entry_contract: "none".to_owned(),
                output: "fixture.ir".into(),
            });
        let mut operations = FakeOperations {
            fail: Some("interfaces"),
            ..FakeOperations::default()
        };

        let report = run(
            &project,
            ProjectAnalysisRequest::default(),
            ProjectAnalysisInputs {
                run_spec: true,
                memory_map: false,
                event_replays: false,
                event_replays_require_interfaces: false,
            },
            &mut operations,
        );

        assert!(!operations.calls.contains(&"ir"));
        let linked_ir = report
            .stages
            .iter()
            .find(|stage| stage.name == "linked-ir")
            .unwrap();
        assert_eq!(linked_ir.status, "blocked");
        assert_eq!(
            linked_ir.reason.as_deref(),
            Some("interface-discovery did not complete")
        );
    }

    #[test]
    fn pipeline_fails_when_an_input_is_aba_rebound_between_producer_and_consumer() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-project-analysis-generation-race-{}",
            std::process::id()
        ));
        if directory.is_dir() {
            std::fs::remove_dir_all(&directory).unwrap();
        }
        std::fs::create_dir_all(&directory).unwrap();
        let input = directory.join("vendor.bin");
        let saved = directory.join("vendor.saved.bin");
        let replacement = directory.join("vendor.replacement.bin");
        std::fs::write(&input, "artifact-a").unwrap();
        std::fs::write(&replacement, "artifact-b").unwrap();

        let mut project = empty_project();
        project.interfaces = Some(crate::project::InterfaceWorkspacePaths {
            facts: "interfaces.json".into(),
            pack: Some("interfaces.toml".into()),
            semantic_catalogs: Vec::new(),
        });
        project
            .ir_profiles
            .push(crate::project_ir::ProjectIrProfile {
                id: "fixture".to_owned(),
                sources: vec!["fixture".to_owned()],
                roots: crate::project_ir::ProjectIrRoots::All,
                include_reachable: true,
                entry_contract: "none".to_owned(),
                output: "fixture.ir".into(),
            });
        let mut operations = FakeOperations {
            pipeline_inputs: Some(
                super::cache::PipelineInputObservation::capture(vec![input.clone()]).unwrap(),
            ),
            rebind_after: Some("interfaces"),
            rebind: Some(InputRebind {
                input: input.clone(),
                saved,
                replacement,
            }),
            ..FakeOperations::default()
        };

        let report = run(
            &project,
            ProjectAnalysisRequest::default(),
            ProjectAnalysisInputs {
                run_spec: true,
                memory_map: false,
                event_replays: false,
                event_replays_require_interfaces: false,
            },
            &mut operations,
        );

        assert!(!report.succeeded());
        assert!(operations.calls.contains(&"interfaces"));
        assert!(!operations.calls.contains(&"ir"));
        let linked_ir = report
            .stages
            .iter()
            .find(|stage| stage.name == "linked-ir")
            .unwrap();
        assert_eq!(linked_ir.status, "failed");
        assert!(
            linked_ir
                .reason
                .as_deref()
                .is_some_and(|reason| reason.contains("changed during analysis"))
        );
        assert_eq!(std::fs::read(&input).unwrap(), b"artifact-a");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn table_free_event_replay_is_independent_of_interface_and_ir_failures() {
        let mut project = empty_project();
        project.interfaces = Some(crate::project::InterfaceWorkspacePaths {
            facts: "interfaces.json".into(),
            pack: Some("interfaces.toml".into()),
            semantic_catalogs: Vec::new(),
        });
        project
            .ir_profiles
            .push(crate::project_ir::ProjectIrProfile {
                id: "fixture".to_owned(),
                sources: vec!["fixture".to_owned()],
                roots: crate::project_ir::ProjectIrRoots::All,
                include_reachable: true,
                entry_contract: "none".to_owned(),
                output: "fixture.ir".into(),
            });
        let mut operations = FakeOperations {
            fail: Some("interfaces"),
            ..FakeOperations::default()
        };

        let report = run(
            &project,
            ProjectAnalysisRequest::default(),
            ProjectAnalysisInputs {
                run_spec: true,
                memory_map: false,
                event_replays: true,
                event_replays_require_interfaces: false,
            },
            &mut operations,
        );

        assert!(operations.calls.contains(&"event-replays"));
        let replay = report
            .stages
            .iter()
            .find(|stage| stage.name == "event-replays")
            .unwrap();
        assert_eq!(replay.status, "written");
    }
}
