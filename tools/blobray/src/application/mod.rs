//! Stateful, CLI-independent application facade for interactive frontends.

pub(crate) mod action;
pub(crate) mod artifact_store;
pub(crate) mod capability_context;
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
pub(crate) mod query_store;
pub(crate) mod research;
mod resolve;
pub(crate) mod revision;
mod snapshot;
pub(crate) mod status;

use std::{collections::BTreeMap, path::Path};

pub(crate) use action::{ExecutableAction, ProjectContextRequirement};
pub use error::{ApplicationError, ApplicationResult};
pub use model::*;
pub use pipeline::StageReport as ProjectAnalysisStageReport;
pub use project_analysis::{
    ProjectAnalysisPlanAction, ProjectAnalysisPlanAwaitingInput, ProjectAnalysisPlanReport,
    ProjectAnalysisPlanStage, ProjectAnalysisPlanWorkItem, ProjectAnalysisReport,
    ProjectAnalysisRequest, ProjectAnalysisStatus,
};
pub(crate) use query_store::{
    QueryStoreCompactionResult as ProjectCacheCompactionResult,
    QueryStoreMaintenancePlan as ProjectCacheMaintenancePlan,
    QueryStorePruneResult as ProjectCachePruneResult,
    QueryStoreRetentionPlan as ProjectCacheRetentionPlan,
    QueryStoreStatistics as ProjectCacheStatistics,
};
pub(crate) use resolve::{
    ExplicitProjectContext, ProjectContext, ProjectSession, ProjectSessionOptions,
};
pub use status::model::{
    AnalysisSurfaceDetail, ArtifactDetail, Component as ProjectStatusComponent, DetailValue,
    EvidenceFreshness, LinkedIrProfileDetail, MmioRegionDetail, Phase as ProjectStatusPhase,
    ProjectStatusReport, Readiness, ResearchCompleteness, ResearchProgress, ReviewScopeDetail,
    StatusValidation, TargetIdentity as ProjectTargetIdentity, ValidationDepth,
};

/// Resolved project state and reload-scoped analysis caches.
///
/// This type never writes to CLI stdout and never parses rendered command
/// output. Frontends receive the same typed reports used by JSON renderers.
pub struct BlobrayApplication {
    resolved: ProjectSession,
    generation: u64,
    analysis_cache: BTreeMap<AnalyzeRequest, AnalysisReport>,
}

impl BlobrayApplication {
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

    /// Execute the complete project-owned analysis/review pipeline.
    pub fn project_analysis(
        &mut self,
        request: ProjectAnalysisRequest,
    ) -> ApplicationResult<ProjectAnalysisReport> {
        let request = request.validate()?;
        let report = project_analysis::analyze_project(&self.resolved, request);
        if !request.check {
            // A write run may publish only part of a failed pipeline or restore
            // CAS outputs while reporting the owning stage as current. Reload
            // unconditionally so snapshots and focused queries never retain
            // pre-run OnceLocks or memoized analysis results.
            let mut resolved = ProjectSession::open(&self.resolved.manifest)?;
            std::mem::swap(&mut self.resolved, &mut resolved);
            self.generation = self.generation.saturating_add(1);
            self.analysis_cache.clear();
        }
        Ok(report)
    }

    /// Compute the exact read-only execution/cache plan for project analysis.
    pub fn project_analysis_plan(
        &self,
        request: ProjectAnalysisRequest,
    ) -> ApplicationResult<ProjectAnalysisPlanReport> {
        let request = request.validate()?;
        Ok(project_analysis::plan_project(&self.resolved, request))
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

pub(crate) fn register_detail_for_project(
    project: &crate::ProjectSpec,
    catalog: &crate::MmioMap,
    address: u32,
) -> crate::Result<Option<RegisterDetailSummary>> {
    snapshot::registers::detail(project, catalog, address)
}

/// Inspect the project-owned persistent cache without creating or repairing it.
pub(crate) fn project_cache_statistics(manifest: &Path) -> crate::Result<ProjectCacheStatistics> {
    query_store::QueryStore::statistics(manifest)
}

/// Inspect reachability, quota and disk-space constraints without mutating the
/// project-owned persistent cache.
pub(crate) fn project_cache_maintenance_plan(
    manifest: &Path,
    max_size_bytes: Option<u64>,
) -> crate::Result<ProjectCacheMaintenancePlan> {
    query_store::QueryStore::maintenance_plan(manifest, max_size_bytes)
}

/// Compact the project-owned persistent cache without evicting live results.
pub(crate) fn compact_project_cache(
    manifest: &Path,
    max_size_bytes: Option<u64>,
) -> crate::Result<ProjectCacheCompactionResult> {
    query_store::QueryStore::compact_cache(manifest, max_size_bytes)
}

/// Inspect persisted unreachable-object retention without mutating the cache.
pub(crate) fn project_cache_retention_plan(
    manifest: &Path,
    retention: std::time::Duration,
    max_size_bytes: Option<u64>,
) -> crate::Result<ProjectCacheRetentionPlan> {
    query_store::QueryStore::retention_plan(manifest, retention, max_size_bytes)
}

/// Prune only persisted unreachable objects older than the requested age.
pub(crate) fn prune_project_cache(
    manifest: &Path,
    retention: std::time::Duration,
    max_size_bytes: Option<u64>,
) -> crate::Result<ProjectCachePruneResult> {
    query_store::QueryStore::prune_cache(manifest, retention, max_size_bytes)
}
