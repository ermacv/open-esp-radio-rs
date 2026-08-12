//! Stateful, CLI-independent application facade for interactive frontends.

mod comparison;
mod error;
pub(crate) mod event_replay;
pub(crate) mod generated_file;
mod model;
mod operations;
pub(crate) mod pipeline;
pub(crate) mod project_analysis;
pub(crate) mod project_files;
pub(crate) mod project_ir_build;
pub(crate) mod project_publication;
mod resolve;
mod snapshot;
pub(crate) mod status;

use std::{collections::BTreeMap, path::Path};

pub use error::{ApplicationError, ApplicationResult};
pub use model::*;
pub(crate) use resolve::{ProjectContext, ProjectSession, ProjectSessionOptions};
pub use status::model::{
    ArtifactDetail, Component as ProjectStatusComponent, DetailValue, LinkedIrProfileDetail,
    MmioRegionDetail, Phase as ProjectStatusPhase, ProjectStatusReport, Readiness,
    ReviewScopeDetail, TargetIdentity as ProjectTargetIdentity,
};

/// Resolved project state and reload-scoped analysis caches.
///
/// This type never writes to CLI stdout and never parses rendered command
/// output. Frontends receive the same typed reports used by JSON renderers.
pub struct WorkbenchApplication {
    resolved: ProjectSession,
    generation: u64,
    analysis_cache: BTreeMap<AnalyzeRequest, AnalysisReport>,
}

impl WorkbenchApplication {
    pub fn open(manifest: &Path) -> ApplicationResult<Self> {
        Ok(Self {
            resolved: ProjectSession::open(manifest)?,
            generation: 1,
            analysis_cache: BTreeMap::new(),
        })
    }

    pub fn snapshot(&mut self) -> ApplicationResult<WorkspaceSnapshot> {
        Ok(snapshot::collect(&self.resolved, self.generation))
    }

    /// Load the heavyweight reviewed projection for one stable function identity.
    pub fn function_detail(
        &self,
        identity: &str,
    ) -> ApplicationResult<Option<FunctionDetailSummary>> {
        Ok(snapshot::function_detail(&self.resolved, identity)?)
    }

    /// Load heavyweight discovery, review and linked-IR evidence for one MMIO address.
    pub fn register_detail(
        &self,
        address: u32,
    ) -> ApplicationResult<Option<RegisterDetailSummary>> {
        Ok(snapshot::register_detail(&self.resolved, address)?)
    }

    pub fn analyze(&mut self, request: AnalyzeRequest) -> ApplicationResult<AnalysisReport> {
        if let Some(report) = self.analysis_cache.get(&request) {
            return Ok(report.clone());
        }
        let report = operations::analyze(&self.resolved, &request)?;
        self.analysis_cache.insert(request, report.clone());
        Ok(report)
    }

    pub fn compare(
        &mut self,
        request: CompareRequest,
    ) -> ApplicationResult<crate::ExecutionComparisonReport> {
        Ok(operations::compare(&self.resolved, request)?)
    }

    /// Execute one checked-in project comparison profile using artifact
    /// bindings from the caller-owned run spec.
    pub fn compare_profile(
        &mut self,
        name: &str,
    ) -> ApplicationResult<crate::ExecutionComparisonReport> {
        comparison::compare_profile(&self.resolved, name)
    }

    pub fn reload(&mut self) -> ApplicationResult<WorkspaceSnapshot> {
        let mut resolved = ProjectSession::open(&self.resolved.manifest)?;
        std::mem::swap(&mut self.resolved, &mut resolved);
        self.generation = self.generation.saturating_add(1);
        self.analysis_cache.clear();
        self.snapshot()
    }

    pub fn manifest(&self) -> &Path {
        &self.resolved.manifest
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture_manifest() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../verification/vendor/targets/esp32s31/vendor-project.toml")
    }

    #[test]
    fn snapshot_is_typed_read_only_state_and_reload_advances_generation() {
        let mut application = WorkbenchApplication::open(&fixture_manifest()).unwrap();
        let first = application.snapshot().unwrap();
        assert_eq!(first.generation, 1);
        assert_eq!(first.project_status.project_id, "esp32s31-radio-rev0");
        assert_eq!(first.project_status.target.architecture, "riscv32");
        assert!(!first.project_status.phases.is_empty());
        assert!(first.registers.configured);
        assert!(!first.registers.registers.is_empty());
        let register = &first.registers.registers[0];
        match application.register_detail(register.address) {
            Ok(Some(register_detail)) => {
                assert_eq!(register_detail.address, register.address);
                assert_eq!(register_detail.name_source, RegisterNameSource::Model);
                assert!(matches!(
                    register_detail.review_status,
                    RegisterReviewState::Manual | RegisterReviewState::Reviewed
                ));
                assert!(register_detail.width.is_some());
            }
            Err(error) => {
                assert!(error.to_string().contains("expected schema_version"));
                assert!(
                    first
                        .diagnostics
                        .iter()
                        .any(|diagnostic| diagnostic.message.contains("expected schema_version"))
                );
            }
            Ok(None) => panic!("catalog register has detail"),
        }
        assert!(!first.comparisons.is_empty());
        // The checked-in project may be opened beside caller-owned local.toml
        // and ignored generated facts during development. Snapshot semantics
        // must stay typed in both an initialized and an analyzed workspace.
        serde_json::to_value(&first).unwrap();
        assert!(
            serde_json::to_value(&first)
                .unwrap()
                .get("function_details")
                .is_none(),
            "workspace index must not eagerly contain heavyweight function details"
        );

        let second = application.reload().unwrap();
        assert_eq!(second.generation, 2);
        assert_eq!(
            second.project_status.project_id,
            first.project_status.project_id
        );
    }
}
