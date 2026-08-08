//! Stateful, CLI-independent application facade for interactive frontends.

mod error;
pub(crate) mod generated_file;
mod model;
mod operations;
pub(crate) mod pipeline;
pub(crate) mod project_analysis;
pub(crate) mod project_publication;
mod resolve;
mod snapshot;
pub(crate) mod status;

use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
};

pub use error::{ApplicationError, ApplicationResult};
pub use model::*;
pub(crate) use resolve::{ProjectContext, ProjectSession, ProjectSessionOptions};

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
        let workspace = self.resolved.project.verification.as_ref().ok_or_else(|| {
            crate::Error::invalid("project has no [verification] profile workspace")
        })?;
        let mut selected = None;
        for path in &workspace.profiles {
            for profile in crate::verification::profiles::load(path)? {
                if profile.name == name && selected.replace(profile).is_some() {
                    return Err(ApplicationError::from(crate::Error::invalid(format!(
                        "comparison profile {name:?} is defined more than once"
                    ))));
                }
            }
        }
        let profile = selected
            .ok_or_else(|| crate::Error::invalid(format!("unknown comparison profile {name:?}")))?;
        let run = self.resolved.run_spec.as_ref().ok_or_else(|| {
            crate::Error::invalid(
                "comparison requires a project run-spec with vendor and Rust artifacts",
            )
        })?;
        let input = |wanted: crate::run_spec::InputRole| -> Option<PathBuf> {
            run.inputs()
                .iter()
                .find(|input| input.role == wanted)
                .map(|input| input.path.clone())
        };
        let source: crate::source_id::SourceId = profile.vendor_source.parse().map_err(|_| {
            crate::Error::invalid(format!(
                "comparison profile {} has invalid source {:?}",
                profile.name, profile.vendor_source
            ))
        })?;
        let vendor_artifact = input(crate::run_spec::InputRole::SourceArtifact(source.clone()))
            .or_else(|| {
                (profile.vendor_source == "vendor")
                    .then(|| input(crate::run_spec::InputRole::VendorArtifact))
                    .flatten()
            })
            .ok_or_else(|| {
                crate::Error::invalid(format!(
                    "run-spec has no artifact for source {}",
                    profile.vendor_source
                ))
            })?;
        let vendor_companion =
            input(crate::run_spec::InputRole::SourceCompanion(source)).or_else(|| {
                (profile.vendor_source == "vendor")
                    .then(|| input(crate::run_spec::InputRole::VendorCompanion))
                    .flatten()
            });
        let rust_artifact = input(crate::run_spec::InputRole::RustArtifact)
            .ok_or_else(|| crate::Error::invalid("run-spec has no rust-artifact input"))?;
        let rust_companion = input(crate::run_spec::InputRole::RustCompanion);
        let table_scenarios = profile
            .scenarios
            .iter()
            .map(|scenario| ComparisonScenario {
                name: scenario.name.clone(),
                scenario: scenario.scenario.clone(),
                vendor_table_instances: scenario.vendor_table_instances.clone(),
                rust_table_instances: scenario.rust_table_instances.clone(),
            })
            .collect::<Vec<_>>();
        operations::validate_table_instances(&self.resolved, &table_scenarios)?;
        let argument_domain = profile.coverage_argument_constraints();
        Ok(crate::compare_execution_scenarios(
            &self.resolved.mmio,
            crate::ExecutionInput {
                artifact: &vendor_artifact,
                companion: vendor_companion.as_deref(),
                symbol: &profile.vendor_symbol,
            },
            crate::ExecutionInput {
                artifact: &rust_artifact,
                companion: rust_companion.as_deref(),
                symbol: &profile.rust_symbol,
            },
            profile.compare_return,
            &argument_domain,
            &profile.scenarios,
        )?)
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
        assert_eq!(first.project_status.architecture, "riscv32");
        assert!(!first.project_status.phases.is_empty());
        assert!(first.registers.configured);
        assert!(!first.registers.registers.is_empty());
        assert!(!first.comparisons.is_empty());
        let comparison_error = application
            .compare_profile(&first.comparisons[0].name)
            .unwrap_err();
        assert!(comparison_error.to_string().contains("run-spec"));
        assert!(
            first
                .diagnostics
                .iter()
                .any(|diagnostic| diagnostic.message.contains("not been generated"))
        );
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
