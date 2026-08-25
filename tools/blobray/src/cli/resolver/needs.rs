//! Positive resource requirements for every CLI command.

use crate::{application::revision::LIVE_REVISION_SELECTOR, cli::args::Command};

/// Resources and capabilities needed before a typed command can be resolved.
///
/// Keeping this as one positive, exhaustive classification prevents the
/// independent deny-lists that previously drifted whenever a command was
/// added or moved between workflows.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub(super) struct ResolutionNeeds {
    pub(super) project: bool,
    pub(super) backend: bool,
    pub(super) knowledge_provider: bool,
    pub(super) knowledge_provider_if_configured: bool,
    pub(super) mmio_map: bool,
    pub(super) memory_map: bool,
    pub(super) register_catalog: bool,
    pub(super) run_spec: bool,
}

impl ResolutionNeeds {
    pub(super) const fn new(
        project: bool,
        backend: bool,
        knowledge_provider: bool,
        mmio_map: bool,
        memory_map: bool,
        register_catalog: bool,
        run_spec: bool,
    ) -> Self {
        Self {
            project,
            backend,
            knowledge_provider,
            knowledge_provider_if_configured: false,
            mmio_map,
            memory_map,
            register_catalog,
            run_spec,
        }
    }

    pub(super) const fn with_configured_knowledge_provider(mut self) -> Self {
        self.knowledge_provider_if_configured = true;
        self
    }

    pub(super) const fn requires_knowledge_provider(self, configured: bool) -> bool {
        self.knowledge_provider || (self.knowledge_provider_if_configured && configured)
    }

    pub(super) fn for_command(command: &Command) -> Self {
        match command {
            Command::GenerateCompletions(_)
            | Command::GenerateManpage(_)
            | Command::ProjectInit(_)
            | Command::ProjectConfigure(_)
            | Command::ProjectInputsInit(_)
            | Command::ProjectBrowse(_)
            | Command::VerifyEvidence(_) => {
                Self::new(false, false, false, false, false, false, false)
            }

            Command::ProjectDoctor(_) => Self::new(true, false, false, false, true, true, true),
            Command::ProjectFiles(_)
            | Command::ProjectCacheStats(_)
            | Command::ProjectCacheGc(_)
            | Command::ProjectCacheCompact(_) => {
                Self::new(true, false, false, false, false, false, false)
            }
            Command::RevisionDiff(arguments) => Self::new(
                true,
                false,
                false,
                false,
                false,
                false,
                revision_operands_need_live_bindings(&arguments.from, &arguments.to),
            ),
            Command::RevisionRebase(arguments) => Self::new(
                true,
                false,
                false,
                false,
                false,
                false,
                revision_operands_need_live_bindings(&arguments.from, &arguments.to),
            ),
            Command::RevisionSnapshot(_) | Command::RevisionPrepareUpdate(_) => {
                Self::new(true, false, false, false, false, false, true)
            }
            Command::ResearchNext(_) => Self::new(true, false, false, false, false, false, false)
                .with_configured_knowledge_provider(),
            Command::ProjectAuditBindings(_) => {
                Self::new(true, false, true, false, false, false, false)
            }
            Command::ProjectStatus(_) => Self::new(true, false, false, false, true, false, true),
            Command::ProjectAnalyze(_) => Self::new(true, true, false, false, true, true, true)
                .with_configured_knowledge_provider(),
            Command::ProjectVerify(_) => Self::new(true, true, false, true, true, true, true)
                .with_configured_knowledge_provider(),
            Command::ProjectCheck(_) => Self::new(true, true, false, true, true, true, true)
                .with_configured_knowledge_provider(),
            Command::ProjectPublish(_) => Self::new(true, false, false, false, true, false, false),

            Command::FunctionInitPack(_)
            | Command::FunctionValidate(_)
            | Command::CodeInitPack(_)
            | Command::CodeRebase(_)
            | Command::CodeValidate(_)
            | Command::CodeReview(_)
            | Command::InterfaceInitPack(_)
            | Command::RegisterReview(_)
            | Command::RegisterExportSvd(_)
            | Command::RegisterGeneratePacRaw(_)
            | Command::RegisterGenerateBindings(_) => {
                Self::new(true, false, false, false, false, false, false)
            }
            Command::FunctionReview(_) | Command::InterfaceValidate(_) => {
                Self::new(true, false, false, false, false, false, false)
                    .with_configured_knowledge_provider()
            }
            Command::RegisterInitModel(_) | Command::RegisterImportSvd(_) => {
                Self::new(true, false, false, false, true, false, false)
            }
            Command::RegisterValidate(_) => Self::new(true, false, false, false, true, true, false),

            Command::SymbolInventory(_)
            | Command::InterfaceDiscover(_)
            | Command::AuditImageTargets(_) => {
                Self::new(false, true, false, false, false, false, true)
            }
            Command::DiscoverMmio(_) => Self::new(false, true, false, false, true, true, true),
            Command::ExportIr(_) => Self::new(false, true, false, false, true, true, true)
                .with_configured_knowledge_provider(),
            Command::BuildIr(_) => Self::new(true, true, false, false, true, true, true)
                .with_configured_knowledge_provider(),

            Command::ExecuteRun(_)
            | Command::ExecuteReplay(_)
            | Command::ExecuteCompare(_)
            | Command::VerifyProfiles(_)
            | Command::InspectTrace(_)
            | Command::InspectCompare(_) => Self::new(false, true, false, true, true, true, true),
            Command::InspectFunction(_) => Self::new(true, true, false, false, false, false, true),
            Command::InspectFlow(_) => Self::new(true, false, false, false, false, false, false),
            Command::InspectObject(_) => Self::new(true, false, false, false, false, false, false),
            Command::InspectRegister(_) => Self::new(true, false, false, false, true, false, false),
            Command::InspectScope(_) => Self::new(true, false, false, false, false, false, false),
            Command::GenerateReference(_)
            | Command::GenerateReferenceBatch(_)
            | Command::InspectAnalyze(_) => Self::new(true, true, true, true, true, true, true),
            Command::VerifyInventory(_) | Command::VerifySource(_) => {
                Self::new(true, true, false, true, true, true, true)
                    .with_configured_knowledge_provider()
            }
        }
    }
}

fn revision_operands_need_live_bindings(from: &str, to: &str) -> bool {
    from == LIVE_REVISION_SELECTOR || to == LIVE_REVISION_SELECTOR
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cache_stats_needs_only_a_project_manifest() {
        let expected = ResolutionNeeds::new(true, false, false, false, false, false, false);
        assert_eq!(
            ResolutionNeeds::for_command(&Command::ProjectCacheStats(Default::default())),
            expected
        );
        assert_eq!(
            ResolutionNeeds::for_command(&Command::ProjectCacheGc(Default::default())),
            expected
        );
        assert_eq!(
            ResolutionNeeds::for_command(&Command::ProjectCacheCompact(Default::default())),
            expected
        );
    }

    #[test]
    fn only_live_revision_operands_require_caller_owned_bindings() {
        assert!(!revision_operands_need_live_bindings("old", "new"));
        assert!(revision_operands_need_live_bindings("old", "@live"));
        assert!(revision_operands_need_live_bindings("@live", "new"));
    }
}
