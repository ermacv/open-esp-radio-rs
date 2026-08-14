//! Typed aggregate produced by the project-owned verification workflow.

use serde::Serialize;

use std::path::Path;

use super::{ReplacementGraph, RustArtifactInput, RustComponentIndex, VerificationCommandReport};

pub(crate) const PROJECT_VERIFICATION_REPORT_SCHEMA: u32 = 14;

#[derive(Serialize)]
pub(crate) struct ProjectVerificationSuiteReport {
    pub(crate) id: String,
    #[serde(flatten)]
    pub(crate) verification: VerificationCommandReport,
}

#[derive(Serialize)]
pub(crate) struct ProjectVerificationReport {
    pub(crate) schema_version: u32,
    pub(crate) command: &'static str,
    pub(crate) project: String,
    pub(crate) passed: bool,
    pub(crate) complete_project_run: bool,
    pub(crate) replacement_graph: ReplacementGraph,
    pub(crate) rust_component_index: RustComponentIndex,
    pub(crate) suites: Vec<ProjectVerificationSuiteReport>,
}

impl ProjectVerificationReport {
    pub(crate) fn new(
        project: String,
        passed: bool,
        complete_project_run: bool,
        suites: Vec<ProjectVerificationSuiteReport>,
        project_manifest: &Path,
        rust_artifacts: &[RustArtifactInput],
    ) -> crate::Result<Self> {
        let replacement_graph = ReplacementGraph::from_suites(&suites)?;
        let rust_component_index = RustComponentIndex::build(
            project_manifest,
            &replacement_graph.components,
            rust_artifacts,
        )?;
        // Each suite gate owns whether its observations are blocking.
        // Reapplying a global zero-DIFF/zero-INCOMPLETE rule here would make
        // `informational` suites indistinguishable from completion gates.
        // The replacement graph still preserves every such observation.
        let passed = project_gate_passes(
            passed,
            rust_component_index.stale_components().is_empty(),
            rust_component_index.stale_artifacts().is_empty(),
        );
        Ok(Self {
            schema_version: PROJECT_VERIFICATION_REPORT_SCHEMA,
            command: "project verify",
            project,
            passed,
            complete_project_run,
            replacement_graph,
            rust_component_index,
            suites,
        })
    }
}

const fn project_gate_passes(
    suite_gates_passed: bool,
    components_fresh: bool,
    artifacts_fresh: bool,
) -> bool {
    suite_gates_passed && components_fresh && artifacts_fresh
}

#[cfg(test)]
mod tests {
    use super::project_gate_passes;

    #[test]
    fn aggregate_respects_suite_gate_classification() {
        assert!(project_gate_passes(true, true, true));
        assert!(!project_gate_passes(false, true, true));
    }

    #[test]
    fn stale_compiled_evidence_always_blocks_the_project() {
        assert!(!project_gate_passes(true, false, true));
        assert!(!project_gate_passes(true, true, false));
    }
}
