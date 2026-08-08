//! Frontend-neutral orchestration of project-owned analysis and review stages.

mod operations;

pub(crate) use operations::*;

use std::path::Path;

use serde::Serialize;

use super::pipeline::{PipelineSummary, StageOutcome, StageSuccess, WorkflowMode, execute};
use crate::{Result, project::ProjectSpec};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProjectAnalysisRequest {
    pub(crate) check: bool,
    pub(crate) deny_unreviewed: bool,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(crate) struct ProjectAnalysisInputs {
    pub(crate) run_spec: bool,
    pub(crate) memory_map: bool,
}

/// Operations required by the project analysis coordinator.
///
/// The coordinator owns ordering and dependency policy. Implementations own
/// the concrete analysis engines and artifact publication boundaries.
pub(crate) trait ProjectAnalysisOperations {
    fn symbol_inventory(&mut self, check: bool) -> Result<bool>;
    fn discover_mmio(&mut self, check: bool) -> Result<bool>;
    fn discover_interfaces(&mut self, check: bool) -> Result<bool>;
    fn build_linked_ir(&mut self, check: bool) -> Result<bool>;
    fn build_navigation(&mut self, check: bool) -> Result<bool>;
    fn validate_registers(&mut self, deny_unreviewed: bool) -> Result<bool>;
    fn review_registers(&mut self, check: bool) -> Result<bool>;
    fn validate_functions(&mut self, deny_unreviewed: bool) -> Result<bool>;
    fn review_functions(&mut self, check: bool) -> Result<bool>;
    fn validate_interfaces(&mut self, deny_unreviewed: bool) -> Result<bool>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ProjectAnalysisReport {
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

impl ProjectAnalysisReport {
    pub(crate) const fn succeeded(&self) -> bool {
        self.failed == 0 && self.blocked == 0
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
        None => StageOutcome::NotConfigured("[analysis.symbols] is absent".to_owned()),
        Some(_) if !inputs.run_spec => {
            StageOutcome::Blocked("run-spec is not configured".to_owned())
        }
        Some(_) => execute("symbol-inventory", generated, || {
            operations.symbol_inventory(mode.is_check())
        }),
    };
    summary.record("symbol-inventory", &symbols);

    let mmio = match project.registers.as_ref() {
        None => StageOutcome::NotConfigured("[registers] is absent".to_owned()),
        Some(_) if !inputs.run_spec => {
            StageOutcome::Blocked("run-spec is not configured".to_owned())
        }
        Some(_) if !inputs.memory_map => {
            StageOutcome::Blocked("memory-map is not configured".to_owned())
        }
        Some(_) => execute("mmio-discovery", generated, || {
            operations.discover_mmio(mode.is_check())
        }),
    };
    summary.record("mmio-discovery", &mmio);

    let interfaces = match project.interfaces.as_ref() {
        None => StageOutcome::NotConfigured("[interfaces] is absent".to_owned()),
        Some(_) if !inputs.run_spec => {
            StageOutcome::Blocked("run-spec is not configured".to_owned())
        }
        Some(_) => execute("interface-discovery", generated, || {
            operations.discover_interfaces(mode.is_check())
        }),
    };
    summary.record("interface-discovery", &interfaces);

    let ir = if project.ir_profiles.is_empty() {
        StageOutcome::NotConfigured("[[analysis.ir]] is absent".to_owned())
    } else if !inputs.run_spec {
        StageOutcome::Blocked("run-spec is not configured".to_owned())
    } else {
        execute("linked-ir", generated, || {
            operations.build_linked_ir(mode.is_check())
        })
    };
    summary.record("linked-ir", &ir);

    let navigation = match project.navigation_index.as_ref() {
        None => StageOutcome::NotConfigured("[analysis.navigation] is absent".to_owned()),
        Some(_) if symbols.blocks_dependants() => {
            StageOutcome::Blocked("symbol-inventory did not complete".to_owned())
        }
        Some(_) if !project.ir_profiles.is_empty() && ir.blocks_dependants() => {
            StageOutcome::Blocked("linked-ir did not complete".to_owned())
        }
        Some(_) if project.interfaces.is_some() && interfaces.blocks_dependants() => {
            StageOutcome::Blocked("interface-discovery did not complete".to_owned())
        }
        Some(_) => execute("navigation-index", generated, || {
            operations.build_navigation(mode.is_check())
        }),
    };
    summary.record("navigation-index", &navigation);

    let register_validation = match project.registers.as_ref() {
        None => StageOutcome::NotConfigured("[registers] is absent".to_owned()),
        Some(_) if mmio.blocks_dependants() => {
            StageOutcome::Blocked("mmio-discovery did not complete".to_owned())
        }
        Some(_) => execute("register-validation", StageSuccess::Verified, || {
            operations.validate_registers(request.deny_unreviewed)
        }),
    };
    summary.record("register-validation", &register_validation);

    let register_review = match project.registers.as_ref() {
        None => StageOutcome::NotConfigured("[registers] is absent".to_owned()),
        Some(paths) if paths.review_output.is_none() => {
            StageOutcome::NotConfigured("[registers.review] is absent".to_owned())
        }
        Some(_) if mmio.blocks_dependants() => {
            StageOutcome::Blocked("mmio-discovery did not complete".to_owned())
        }
        Some(paths)
            if review_depends_on_project_ir(project, &paths.review_ir_reports)
                && ir.blocks_dependants() =>
        {
            StageOutcome::Blocked("linked-ir did not complete".to_owned())
        }
        Some(_) => execute("register-review", generated, || {
            operations.review_registers(mode.is_check())
        }),
    };
    summary.record("register-review", &register_review);

    let function_validation = match project.functions.as_ref() {
        None => StageOutcome::NotConfigured("[functions] is absent".to_owned()),
        Some(_) if ir.blocks_dependants() => {
            StageOutcome::Blocked("linked-ir did not complete".to_owned())
        }
        Some(_) => execute("function-validation", StageSuccess::Verified, || {
            operations.validate_functions(request.deny_unreviewed)
        }),
    };
    summary.record("function-validation", &function_validation);

    let function_review = match project.functions.as_ref() {
        None => StageOutcome::NotConfigured("[functions] is absent".to_owned()),
        Some(paths) if paths.review_output.is_none() => {
            StageOutcome::NotConfigured("[functions.review] is absent".to_owned())
        }
        Some(_) if ir.blocks_dependants() => {
            StageOutcome::Blocked("linked-ir did not complete".to_owned())
        }
        Some(_)
            if project
                .interfaces
                .as_ref()
                .and_then(|paths| paths.pack.as_deref())
                .is_some_and(Path::is_file)
                && interfaces.blocks_dependants() =>
        {
            StageOutcome::Blocked("interface-discovery did not complete".to_owned())
        }
        Some(_) => execute("function-review", generated, || {
            operations.review_functions(mode.is_check())
        }),
    };
    summary.record("function-review", &function_review);

    let interface_validation = match project.interfaces.as_ref() {
        None => StageOutcome::NotConfigured("[interfaces] is absent".to_owned()),
        Some(paths) if paths.pack.is_none() => {
            StageOutcome::NotConfigured("[interfaces].pack is absent".to_owned())
        }
        Some(_) if interfaces.blocks_dependants() => {
            StageOutcome::Blocked("interface-discovery did not complete".to_owned())
        }
        Some(_) => execute("interface-validation", StageSuccess::Verified, || {
            operations.validate_interfaces(request.deny_unreviewed)
        }),
    };
    summary.record("interface-validation", &interface_validation);

    ProjectAnalysisReport {
        schema: 1,
        command: "project analyze",
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

fn review_depends_on_project_ir(project: &ProjectSpec, reports: &[std::path::PathBuf]) -> bool {
    project
        .ir_profiles
        .iter()
        .any(|profile| reports.iter().any(|report| report == &profile.output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::project_analysis::SymbolInventorySpec;

    #[derive(Default)]
    struct FakeOperations {
        calls: Vec<&'static str>,
    }

    impl FakeOperations {
        fn called(&mut self, name: &'static str) -> Result<bool> {
            self.calls.push(name);
            Ok(true)
        }
    }

    impl ProjectAnalysisOperations for FakeOperations {
        fn symbol_inventory(&mut self, _: bool) -> Result<bool> {
            self.called("symbols")
        }

        fn discover_mmio(&mut self, _: bool) -> Result<bool> {
            self.called("mmio")
        }

        fn discover_interfaces(&mut self, _: bool) -> Result<bool> {
            self.called("interfaces")
        }

        fn build_linked_ir(&mut self, _: bool) -> Result<bool> {
            self.called("ir")
        }

        fn build_navigation(&mut self, _: bool) -> Result<bool> {
            self.called("navigation")
        }

        fn validate_registers(&mut self, _: bool) -> Result<bool> {
            self.called("register-validation")
        }

        fn review_registers(&mut self, _: bool) -> Result<bool> {
            self.called("register-review")
        }

        fn validate_functions(&mut self, _: bool) -> Result<bool> {
            self.called("function-validation")
        }

        fn review_functions(&mut self, _: bool) -> Result<bool> {
            self.called("function-review")
        }

        fn validate_interfaces(&mut self, _: bool) -> Result<bool> {
            self.called("interface-validation")
        }
    }

    fn empty_project() -> ProjectSpec {
        ProjectSpec {
            id: "fixture".to_owned(),
            target_spec: "target.spec".into(),
            platform_pack: None,
            run_spec: None,
            memory_map: None,
            svd_configured: false,
            svd_paths: Vec::new(),
            symbol_inventory: None,
            navigation_index: None,
            ir_profiles: Vec::new(),
            registers: None,
            interfaces: None,
            functions: None,
            verification: None,
        }
    }

    #[test]
    fn optional_absence_does_not_run_operations_or_fail_the_workflow() {
        let mut operations = FakeOperations::default();
        let report = run(
            &empty_project(),
            ProjectAnalysisRequest::default(),
            ProjectAnalysisInputs::default(),
            &mut operations,
        );
        assert!(operations.calls.is_empty());
        assert!(report.succeeded());
        assert_eq!(report.not_configured, 10);
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
    }
}
