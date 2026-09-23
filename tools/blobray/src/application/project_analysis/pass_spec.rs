//! One owner for project pass identities, dependencies, execution and cache semantics.
//!
//! A resolved graph is built once and shared by run and plan. Inputs/outputs of
//! individual work items remain owned by domain operations; this graph does not
//! yet make their generated-file publication an atomic project snapshot.

use super::{ProjectAnalysisInputs, ProjectAnalysisOperations, ProjectAnalysisRequest};
use crate::application::pipeline::{StageRun, StageSuccess, WorkflowMode};
use crate::{Result, project::ProjectSpec};

macro_rules! passes {
    ($( $kind:ident { $( $field:ident: $value:expr ),* $(,)? } ),* $(,)?) => {
        #[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
        pub(super) enum PassKind { $( $kind, )* }
        pub(super) const PASSES: &[PassSpec] = &[
            $(PassSpec { kind: PassKind::$kind, $( $field: $value, )* },)*
        ];
    };
}

pub(super) struct PassSpec {
    pub(super) kind: PassKind,
    pub(super) name: &'static str,
    pub(super) revision: Option<u32>,
    pub(super) artifact_schema: Option<crate::artifacts::ArtifactSchema>,
    pub(super) analysis_domain: Option<&'static [u8]>,
    pub(super) compiled_knowledge: bool,
    validation: bool,
    needs_run: bool,
    needs_memory: bool,
}

passes! {
    SymbolInventory {
        name: "symbol-inventory",
        revision: Some(4),
        artifact_schema: Some(crate::artifacts::SYMBOL_INVENTORY),
        analysis_domain: None,
        compiled_knowledge: false,
        validation: false,
        needs_run: true,
        needs_memory: false,
    },
    MmioDiscovery {
        name: "mmio-discovery",
        revision: Some(9),
        artifact_schema: Some(crate::artifacts::MMIO_FACTS),
        analysis_domain: None,
        compiled_knowledge: false,
        validation: false,
        needs_run: true,
        needs_memory: true,
    },
    InterfaceDiscovery {
        name: "interface-discovery",
        revision: Some(14),
        artifact_schema: Some(crate::artifacts::INTERFACE_FACTS),
        analysis_domain: None,
        compiled_knowledge: false,
        validation: false,
        needs_run: true,
        needs_memory: false,
    },
    LinkedIr {
        name: "linked-ir",
        revision: Some(69),
        artifact_schema: Some(crate::artifacts::LINKED_IR),
        analysis_domain: Some(crate::analysis::FUNCTION_FACT_CACHE_DOMAIN),
        compiled_knowledge: true,
        validation: false,
        needs_run: true,
        needs_memory: false,
    },
    EventReplays {
        name: "event-replays",
        revision: Some(1),
        artifact_schema: Some(crate::artifacts::REPLAY_EVIDENCE),
        analysis_domain: None,
        compiled_knowledge: true,
        validation: false,
        needs_run: true,
        needs_memory: false,
    },
    ReviewScopes {
        name: "review-scopes",
        revision: Some(5),
        artifact_schema: None,
        analysis_domain: None,
        compiled_knowledge: false,
        validation: false,
        needs_run: false,
        needs_memory: false,
    },
    NavigationIndex {
        name: "navigation-index",
        revision: Some(7),
        artifact_schema: None,
        analysis_domain: None,
        compiled_knowledge: false,
        validation: false,
        needs_run: false,
        needs_memory: false,
    },
    CodeValidation {
        name: "code-boundary-validation",
        revision: Some(1),
        artifact_schema: None,
        analysis_domain: None,
        compiled_knowledge: false,
        validation: true,
        needs_run: false,
        needs_memory: false,
    },
    CodeReview {
        name: "code-boundary-review",
        revision: Some(1),
        artifact_schema: None,
        analysis_domain: None,
        compiled_knowledge: false,
        validation: false,
        needs_run: false,
        needs_memory: false,
    },
    RegisterValidation {
        name: "register-validation",
        revision: Some(1),
        artifact_schema: None,
        analysis_domain: None,
        compiled_knowledge: false,
        validation: true,
        needs_run: false,
        needs_memory: false,
    },
    RegisterReview {
        name: "register-review",
        revision: Some(1),
        artifact_schema: None,
        analysis_domain: None,
        compiled_knowledge: false,
        validation: false,
        needs_run: false,
        needs_memory: false,
    },
    FunctionValidation {
        name: "function-validation",
        revision: Some(2),
        artifact_schema: None,
        analysis_domain: None,
        compiled_knowledge: false,
        validation: true,
        needs_run: false,
        needs_memory: false,
    },
    FunctionReview {
        name: "function-review",
        revision: Some(5),
        artifact_schema: None,
        analysis_domain: None,
        compiled_knowledge: true,
        validation: false,
        needs_run: false,
        needs_memory: false,
    },
    InterfaceValidation {
        name: "interface-validation",
        revision: Some(4),
        artifact_schema: None,
        analysis_domain: None,
        compiled_knowledge: true,
        validation: true,
        needs_run: false,
        needs_memory: false,
    },
    CapabilityContext {
        name: "interface-capability-context",
        revision: Some(1),
        artifact_schema: Some(crate::artifacts::CAPABILITY_CONTEXT),
        analysis_domain: None,
        compiled_knowledge: true,
        validation: false,
        needs_run: false,
        needs_memory: false,
    },
    Coverage {
        name: "analysis-coverage",
        revision: None,
        artifact_schema: None,
        analysis_domain: None,
        compiled_knowledge: false,
        validation: true,
        needs_run: true,
        needs_memory: false,
    },
}

impl PassKind {
    pub(super) fn spec(self) -> &'static PassSpec {
        PASSES
            .iter()
            .find(|spec| spec.kind == self)
            .expect("pass is registered")
    }
}

pub(super) enum CacheWork<'a> {
    Whole,
    Profile(&'a str),
    Validation { deny_unreviewed: bool },
}

/// Checked work-item identity. Unknown owners and unsupported suffixes cannot
/// accidentally inherit another pass's configuration or semantic revision.
pub(super) struct CachePass<'a> {
    pub(super) spec: &'static PassSpec,
    pub(super) work: CacheWork<'a>,
}

impl<'a> CachePass<'a> {
    pub(super) fn parse(key: &'a str) -> Result<Self> {
        let (name, suffix) = key
            .split_once(':')
            .map_or((key, None), |(a, b)| (a, Some(b)));
        let error = || {
            crate::Error::invalid(format!(
                "analysis cache has no semantic revision for stage {key:?}"
            ))
        };
        let spec = PASSES
            .iter()
            .find(|spec| spec.name == name && spec.revision.is_some())
            .ok_or_else(error)?;
        let work = match (spec.kind, spec.validation, suffix) {
            (PassKind::LinkedIr, _, Some(profile)) if !profile.is_empty() => {
                CacheWork::Profile(profile)
            }
            (_, true, Some("deny-unreviewed=true")) => CacheWork::Validation {
                deny_unreviewed: true,
            },
            (_, true, Some("deny-unreviewed=false")) => CacheWork::Validation {
                deny_unreviewed: false,
            },
            (_, false, None) => CacheWork::Whole,
            _ => return Err(error()),
        };
        Ok(Self { spec, work })
    }

    pub(super) fn revision(&self) -> u32 {
        self.spec
            .revision
            .expect("cache key requires versioned pass")
    }

    pub(super) fn cacheable(&self, provider_domain: Option<&str>) -> bool {
        self.spec.kind != PassKind::LinkedIr
            || provider_domain.is_some_and(|domain| !domain.trim().is_empty())
    }

    pub(super) fn semantic_name(&self) -> String {
        match self.work {
            CacheWork::Whole | CacheWork::Profile(_) => self.spec.name.to_owned(),
            CacheWork::Validation { deny_unreviewed } => {
                format!("{}:deny-unreviewed={deny_unreviewed}", self.spec.name)
            }
        }
    }
}

pub(super) enum Availability {
    Ready,
    NotConfigured(&'static str),
    Blocked(&'static str),
}

pub(super) struct ResolvedPass {
    pub(super) spec: &'static PassSpec,
    pub(super) availability: Availability,
    pub(super) dependencies: Vec<PassKind>,
    pub(super) optional_dependencies: Vec<PassKind>,
}

pub(super) struct PassGraph {
    pub(super) nodes: Vec<ResolvedPass>,
}

impl PassGraph {
    pub(super) fn resolve(project: &ProjectSpec, inputs: ProjectAnalysisInputs) -> Self {
        let coverage = !project.ir_profiles.is_empty()
            || project.analysis_symbol_families.iter().any(|family| {
                family.disposition == crate::project::AnalysisSymbolFamilyDisposition::Required
            });
        let nodes = PASSES
            .iter()
            .filter(|spec| spec.kind != PassKind::Coverage || coverage)
            .map(|spec| {
                let availability = if let Some(reason) = spec.not_configured(project, inputs) {
                    Availability::NotConfigured(reason)
                } else if spec.needs_run && !inputs.run_spec {
                    Availability::Blocked(if spec.kind == PassKind::Coverage {
                        "run-spec is not configured; required coverage cannot be established"
                    } else {
                        "run-spec is not configured"
                    })
                } else if spec.needs_memory && !inputs.memory_map {
                    Availability::Blocked("memory-map is not configured")
                } else {
                    Availability::Ready
                };
                ResolvedPass {
                    spec,
                    availability,
                    dependencies: dependencies(project, inputs, spec.kind),
                    optional_dependencies: optional_dependencies(project, spec.kind),
                }
            })
            .collect();
        Self { nodes }
    }

    pub(super) fn node(&self, name: &str) -> Option<&ResolvedPass> {
        self.nodes.iter().find(|node| node.spec.name == name)
    }
}

impl PassSpec {
    fn not_configured(
        &self,
        project: &ProjectSpec,
        inputs: ProjectAnalysisInputs,
    ) -> Option<&'static str> {
        use PassKind::*;
        match self.kind {
            SymbolInventory => project
                .symbol_inventory
                .is_none()
                .then_some("[analysis.symbols] is absent"),
            MmioDiscovery | RegisterValidation => project
                .registers
                .is_none()
                .then_some("[registers] is absent"),
            InterfaceDiscovery => project
                .interfaces
                .is_none()
                .then_some("[interfaces] is absent"),
            LinkedIr => project
                .ir_profiles
                .is_empty()
                .then_some("[[analysis.ir]] is absent"),
            EventReplays => {
                (!inputs.event_replays).then_some("no reviewed event replay is configured")
            }
            ReviewScopes => project.review.is_none().then_some("[review] is absent"),
            NavigationIndex => project
                .navigation_index
                .is_none()
                .then_some("[analysis.navigation] is absent"),
            CodeValidation => project.code.is_none().then_some("[code] is absent"),
            CodeReview => match &project.code {
                None => Some("[code] is absent"),
                Some(paths) if paths.review_output.is_none() => Some("[code.review] is absent"),
                _ => None,
            },
            RegisterReview => match &project.registers {
                None => Some("[registers] is absent"),
                Some(paths) if paths.review_output.is_none() => {
                    Some("[registers.review] is absent")
                }
                _ => None,
            },
            FunctionValidation => project
                .functions
                .is_none()
                .then_some("[functions] is absent"),
            FunctionReview => match &project.functions {
                None => Some("[functions] is absent"),
                Some(paths) if paths.review_output.is_none() => {
                    Some("[functions.review] is absent")
                }
                _ => None,
            },
            InterfaceValidation => match &project.interfaces {
                None => Some("[interfaces] is absent"),
                Some(paths) if paths.pack.is_none() => Some("[interfaces].pack is absent"),
                _ => None,
            },
            CapabilityContext => match &project.interfaces {
                None => Some("[interfaces] is absent"),
                Some(paths) if paths.capability_context.is_none() => {
                    Some("[interfaces].pack is absent")
                }
                _ => None,
            },
            Coverage => None,
        }
    }

    pub(super) fn success(&self, mode: WorkflowMode) -> StageSuccess {
        if self.validation {
            StageSuccess::Verified
        } else {
            mode.generated_success()
        }
    }

    pub(super) fn execute(
        &self,
        request: ProjectAnalysisRequest,
        operations: &mut impl ProjectAnalysisOperations,
    ) -> Result<StageRun> {
        use PassKind::*;
        operations.validate_pipeline_inputs()?;
        match self.kind {
            SymbolInventory => operations.symbol_inventory(request.check),
            MmioDiscovery => operations.discover_mmio(request.check, request.jobs),
            InterfaceDiscovery => operations.discover_interfaces(request.check),
            LinkedIr => operations.build_linked_ir(request.check, request.jobs),
            EventReplays => operations.build_event_replays(request.check),
            ReviewScopes => operations.build_review_scopes(request.check),
            NavigationIndex => operations.build_navigation(request.check),
            CodeValidation => operations.validate_code(request.deny_unreviewed),
            CodeReview => operations.review_code(request.check),
            RegisterValidation => operations.validate_registers(request.deny_unreviewed),
            RegisterReview => operations.review_registers(request.check),
            FunctionValidation => operations.validate_functions(request.deny_unreviewed),
            FunctionReview => operations.review_functions(request.check),
            InterfaceValidation => operations.validate_interfaces(request.deny_unreviewed),
            CapabilityContext => operations.build_capability_context(request.check),
            Coverage => operations.coverage(),
        }
    }
}

pub(super) fn linked_ir_uses_reviewed_interfaces(project: &ProjectSpec) -> bool {
    project
        .interfaces
        .as_ref()
        .is_some_and(|interfaces| interfaces.pack.is_some())
}

fn dependencies(
    project: &ProjectSpec,
    inputs: ProjectAnalysisInputs,
    stage: PassKind,
) -> Vec<PassKind> {
    let mut dependencies = Vec::new();
    if stage == PassKind::Coverage && !project.ir_profiles.is_empty() {
        dependencies.push(PassKind::LinkedIr);
    }
    let mut configured = |name: PassKind, include: bool| {
        if include {
            dependencies.push(name);
        }
    };
    match stage {
        PassKind::MmioDiscovery | PassKind::InterfaceDiscovery => {
            configured(
                PassKind::SymbolInventory,
                project.code.is_some() && project.symbol_inventory.is_some(),
            );
        }
        PassKind::LinkedIr => {
            configured(
                PassKind::SymbolInventory,
                project.code.is_some() && project.symbol_inventory.is_some(),
            );
            configured(
                PassKind::InterfaceDiscovery,
                linked_ir_uses_reviewed_interfaces(project),
            );
        }
        PassKind::EventReplays => {
            configured(
                PassKind::InterfaceDiscovery,
                inputs.event_replays_require_interfaces && project.interfaces.is_some(),
            );
        }
        PassKind::ReviewScopes => {
            configured(PassKind::LinkedIr, !project.ir_profiles.is_empty());
            configured(PassKind::MmioDiscovery, project.registers.is_some());
        }
        PassKind::NavigationIndex => {
            configured(
                PassKind::SymbolInventory,
                project.symbol_inventory.is_some(),
            );
            configured(PassKind::LinkedIr, !project.ir_profiles.is_empty());
            configured(PassKind::InterfaceDiscovery, project.interfaces.is_some());
        }
        PassKind::CodeValidation | PassKind::CodeReview => {
            configured(
                PassKind::SymbolInventory,
                project.symbol_inventory.is_some(),
            );
        }
        PassKind::RegisterValidation => {
            configured(PassKind::MmioDiscovery, project.registers.is_some());
        }
        PassKind::RegisterReview => {
            configured(PassKind::MmioDiscovery, project.registers.is_some());
            configured(
                PassKind::LinkedIr,
                project.registers.as_ref().is_some_and(|registers| {
                    project.ir_profiles.iter().any(|profile| {
                        registers
                            .review_ir_reports
                            .iter()
                            .any(|report| report == &profile.output)
                    })
                }),
            );
        }
        PassKind::FunctionValidation => {
            configured(PassKind::LinkedIr, !project.ir_profiles.is_empty());
        }
        PassKind::FunctionReview => {
            configured(PassKind::LinkedIr, !project.ir_profiles.is_empty());
            configured(
                PassKind::InterfaceDiscovery,
                project
                    .interfaces
                    .as_ref()
                    .and_then(|paths| paths.pack.as_deref())
                    .is_some_and(std::path::Path::is_file),
            );
        }
        PassKind::InterfaceValidation => {
            configured(PassKind::InterfaceDiscovery, project.interfaces.is_some());
        }
        PassKind::CapabilityContext => {
            configured(
                PassKind::InterfaceValidation,
                project
                    .interfaces
                    .as_ref()
                    .is_some_and(|interfaces| interfaces.capability_context.is_some()),
            );
        }
        _ => {}
    }
    dependencies
}

fn optional_dependencies(project: &ProjectSpec, stage: PassKind) -> Vec<PassKind> {
    match stage {
        PassKind::LinkedIr if project.code.is_none() && project.symbol_inventory.is_some() => {
            vec![PassKind::SymbolInventory]
        }
        _ => Vec::new(),
    }
}

pub(super) fn stage_configuration(
    project: &ProjectSpec,
    stage: &str,
    linked_ir_semantic_cache_domain: Option<&str>,
) -> crate::Result<String> {
    let key = CachePass::parse(stage)?;
    if let CacheWork::Profile(profile_id) = key.work {
        let profile = project
            .ir_profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .ok_or_else(|| {
                crate::Error::invalid(format!(
                    "linked-IR cache stage names unconfigured profile {profile_id:?}"
                ))
            })?;
        return Ok(format!(
            "sources={:?};roots={:?};include-reachable={};entry-contract={:?};effective-code-domain={:?};riscv-semantic-cache-domain={:?}",
            profile.sources,
            profile.roots,
            profile.include_reachable,
            profile.entry_contract,
            effective_code_domain(project),
            linked_ir_semantic_cache_domain,
        ));
    }
    Ok(match key.spec.kind {
        PassKind::SymbolInventory => format!("{:?}", project.symbol_inventory),
        PassKind::MmioDiscovery => format!(
            "facts={:?};effective-code-domain={:?}",
            project.registers.as_ref().map(|registers| &registers.facts),
            effective_code_domain(project),
        ),
        PassKind::InterfaceDiscovery => format!(
            "facts={:?};effective-code-domain={:?}",
            project
                .interfaces
                .as_ref()
                .map(|interfaces| &interfaces.facts),
            effective_code_domain(project),
        ),
        PassKind::LinkedIr => format!(
            "profiles={:?};effective-code-domain={:?};riscv-semantic-cache-domain={:?}",
            project.ir_profiles,
            effective_code_domain(project),
            linked_ir_semantic_cache_domain,
        ),
        PassKind::EventReplays => format!("{:?}", project.functions),
        PassKind::ReviewScopes => format!(
            "project-id={:?};review={:?};profile-bindings={:?};policy={:?}",
            project.id,
            project.review,
            review_profile_bindings(project),
            project
                .verification
                .as_ref()
                .and_then(|verification| verification.policy.as_ref())
        ),
        PassKind::NavigationIndex => {
            let profile_bindings = project
                .ir_profiles
                .iter()
                .map(|profile| (profile.id.as_str(), profile.output.as_path()))
                .collect::<Vec<_>>();
            format!(
                "navigation={:?};linked-ir-profile-bindings={profile_bindings:?}",
                project.navigation_index
            )
        }
        PassKind::CodeValidation | PassKind::CodeReview => {
            format!("project-id={:?};code={:?}", project.id, project.code)
        }
        PassKind::RegisterValidation | PassKind::RegisterReview => {
            format!("{:?}", project.registers)
        }
        PassKind::FunctionValidation | PassKind::FunctionReview => format!(
            "functions={:?};profile-bindings={:?}",
            project.functions,
            function_profile_bindings(project)
        ),
        PassKind::InterfaceValidation | PassKind::CapabilityContext => {
            format!("{:?}", project.interfaces)
        }
        PassKind::Coverage => unreachable!("coverage does not have a cache key"),
    })
}

fn effective_code_domain(
    project: &ProjectSpec,
) -> Option<(&str, &crate::project::CodeWorkspacePaths)> {
    project
        .code
        .as_ref()
        .map(|code| (project.id.as_str(), code))
}

fn function_profile_bindings(project: &ProjectSpec) -> Vec<(String, std::path::PathBuf)> {
    project
        .functions
        .as_ref()
        .map(|functions| ir_profile_bindings(project, &functions.profiles))
        .unwrap_or_default()
}

fn review_profile_bindings(project: &ProjectSpec) -> Vec<(String, std::path::PathBuf)> {
    let ids = project
        .review
        .iter()
        .flat_map(|review| &review.scopes)
        .flat_map(|scope| &scope.profiles)
        .cloned()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    ir_profile_bindings(project, &ids)
}

fn ir_profile_bindings(project: &ProjectSpec, ids: &[String]) -> Vec<(String, std::path::PathBuf)> {
    ids.iter()
        .map(|id| {
            let profile = project
                .ir_profiles
                .iter()
                .find(|profile| profile.id == *id)
                .expect("reviewed project profile was validated while loading the manifest");
            (id.clone(), profile.output.clone())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_keys_reject_unregistered_work_without_guessing_an_owner() {
        for key in [
            "unknown",
            "linked-ir:",
            "symbol-inventory:profile",
            "register-validation",
            "register-validation:deny-unreviewed=maybe",
            "register-review:deny-unreviewed=false",
            "analysis-coverage",
        ] {
            assert!(CachePass::parse(key).is_err(), "accepted {key}");
        }
        let first = CachePass::parse("linked-ir:one").unwrap();
        let second = CachePass::parse("linked-ir:two").unwrap();
        assert_eq!(first.semantic_name(), second.semantic_name());
        assert_ne!(
            CachePass::parse("register-validation:deny-unreviewed=true")
                .unwrap()
                .semantic_name(),
            CachePass::parse("register-validation:deny-unreviewed=false")
                .unwrap()
                .semantic_name()
        );
    }
}
