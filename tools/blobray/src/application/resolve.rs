//! Project resolution for non-CLI frontends.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

use crate::{MemoryMap, MmioMap, ProjectSpec, Result, TargetSpec, run_spec::RunSpec};

use super::action::{ExecutableAction, ProjectContextRequirement};
use super::artifact_store::ProjectArtifactStore;

pub(crate) struct ProjectContext<'a> {
    pub(crate) project_path: &'a Path,
    pub(crate) project: &'a ProjectSpec,
    pub(crate) target_path: &'a Path,
    pub(crate) target: &'a TargetSpec,
    pub(crate) run_spec_path: Option<&'a Path>,
    pub(crate) run_spec: Option<&'a RunSpec>,
    pub(crate) memory_map: Option<&'a MemoryMap>,
    pub(crate) svd_paths: &'a [PathBuf],
    pub(crate) svd: &'a MmioMap,
    pub(crate) explicit_context: &'a ExplicitProjectContext,
    pub(crate) invocation_directory: &'a Path,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct ExplicitProjectContext {
    pub(crate) target_spec: Option<PathBuf>,
    pub(crate) run_spec: Option<PathBuf>,
    pub(crate) svd_paths: Vec<PathBuf>,
}

impl ProjectContext<'_> {
    pub(crate) fn follow_up_action<I, S>(
        &self,
        command: I,
        requirement: ProjectContextRequirement,
    ) -> Result<ExecutableAction>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let mut argv = vec!["blobray".to_owned()];
        argv.extend(command.into_iter().map(Into::into));
        push_path_argument(&mut argv, "--project", self.project_path)?;
        if requirement.target_spec()
            && let Some(path) = self.explicit_context.target_spec.as_deref()
        {
            push_path_argument(&mut argv, "--target-spec", path)?;
        }
        if requirement.run_spec()
            && let Some(path) = self.explicit_context.run_spec.as_deref()
        {
            push_path_argument(&mut argv, "--run-spec", path)?;
        }
        if requirement.register_catalog() {
            for path in &self.explicit_context.svd_paths {
                push_path_argument(&mut argv, "--svd", path)?;
            }
        }
        ExecutableAction::new(argv, self.invocation_directory.to_owned(), requirement)
    }

    /// Executable help entry point for repairing caller-owned input bindings.
    ///
    /// An explicit run-spec override is the output destination for this
    /// command, not a resolution root: omitting it would silently edit the
    /// project's default `local.toml` instead.
    pub(crate) fn inputs_init_help_action(&self) -> Result<ExecutableAction> {
        let mut action = self.follow_up_action(
            ["project", "inputs", "init"],
            ProjectContextRequirement::ProjectOnly,
        )?;
        if let Some(path) = self.explicit_context.run_spec.as_deref() {
            push_path_argument(&mut action.argv, "--output", path)?;
        }
        action.argv.push("--help".to_owned());
        Ok(action)
    }
}

fn push_path_argument(argv: &mut Vec<String>, option: &str, path: &Path) -> Result<()> {
    let value = path.to_str().ok_or_else(|| {
        crate::Error::invalid(format!(
            "executable action {option} path is not valid UTF-8 and cannot be serialized"
        ))
    })?;
    argv.push(option.to_owned());
    argv.push(value.to_owned());
    Ok(())
}

pub(crate) struct ProjectSessionOptions {
    pub(crate) target_spec: Option<PathBuf>,
    pub(crate) run_spec: Option<PathBuf>,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) load_run_spec: bool,
    pub(crate) authenticate_review_context: bool,
    pub(crate) load_memory_map: bool,
    pub(crate) load_register_catalog: bool,
    pub(crate) invocation_directory: Option<PathBuf>,
}

impl Default for ProjectSessionOptions {
    fn default() -> Self {
        Self {
            target_spec: None,
            run_spec: None,
            svd_paths: Vec::new(),
            load_run_spec: true,
            authenticate_review_context: true,
            load_memory_map: true,
            load_register_catalog: true,
            invocation_directory: None,
        }
    }
}

pub(crate) struct ProjectSession {
    pub(crate) manifest: PathBuf,
    pub(crate) project: ProjectSpec,
    pub(crate) target_path: PathBuf,
    pub(crate) target: TargetSpec,
    pub(crate) run_spec_path: Option<PathBuf>,
    pub(crate) run_spec: Option<RunSpec>,
    pub(crate) memory_map: Option<MemoryMap>,
    pub(crate) svd_paths: Vec<PathBuf>,
    pub(crate) mmio: MmioMap,
    pub(crate) explicit_context: ExplicitProjectContext,
    pub(crate) invocation_directory: PathBuf,
    pub(crate) function_workspace:
        OnceLock<std::result::Result<Option<crate::function_workspace::FunctionWorkspace>, String>>,
    pub(crate) code_workspace:
        OnceLock<std::result::Result<Option<crate::code_workspace::CodeWorkspace>, String>>,
    pub(crate) register_workspace:
        OnceLock<std::result::Result<Option<crate::registers::ProjectRegisterWorkspace>, String>>,
    pub(crate) interface_workspace:
        OnceLock<std::result::Result<Option<crate::interfaces::InterfaceWorkspace>, String>>,
    pub(crate) artifacts: ProjectArtifactStore,
}

impl ProjectSession {
    pub(crate) fn open(manifest: &Path) -> Result<Self> {
        Self::open_with(manifest, ProjectSessionOptions::default())
    }

    pub(crate) fn open_with(manifest: &Path, options: ProjectSessionOptions) -> Result<Self> {
        let invocation_directory = options
            .invocation_directory
            .clone()
            .map_or_else(std::env::current_dir, Ok)?;
        let manifest = manifest.to_owned();
        let mut project = ProjectSpec::load(&manifest)?;
        let explicit_context = ExplicitProjectContext {
            target_spec: options.target_spec.clone(),
            run_spec: options.run_spec.clone(),
            svd_paths: options.svd_paths.clone(),
        };
        let target_path = options
            .target_spec
            .unwrap_or_else(|| project.target_spec.clone());
        let mut target = TargetSpec::load(&target_path)?;
        project.apply_to_target(&mut target)?;

        let run_spec_path = options
            .load_run_spec
            .then(|| {
                options
                    .run_spec
                    .or_else(|| project.run_spec.clone())
                    .or_else(|| {
                        manifest
                            .parent()
                            .map(|parent| parent.join("local.toml"))
                            .filter(|path| path.is_file())
                    })
            })
            .flatten();
        let run_spec = run_spec_path.as_deref().map(RunSpec::load).transpose()?;
        if options.authenticate_review_context {
            let knowledge =
                open_radio_vendor_review::ReviewKnowledge::load_all(&project.reviewed_knowledge)
                    .map_err(|error| {
                        crate::Error::invalid(format!("cannot load reviewed knowledge: {error}"))
                    })?;
            let sources = knowledge.constrained_artifact_sources();
            if !sources.is_empty() {
                let run_spec = run_spec.as_ref().ok_or_else(|| {
                    crate::Error::invalid(
                        "reviewed facts constrain exact artifact bytes, but this command has no active run spec to authenticate them",
                    )
                })?;
                project.review_context.artifacts =
                    run_spec.artifact_identities_for_sources(&sources)?;
            }
            if let Some(registers) = project.registers.as_mut() {
                registers.review_context = project.review_context.clone();
            }
            knowledge
                .select_for(&project.review_context)
                .map_err(|error| {
                    crate::Error::invalid(format!(
                        "reviewed knowledge does not apply to the authenticated active inputs: {error}"
                    ))
                })?;
        }
        let memory_map_path = project.memory_map.as_deref();
        let memory_map = if options.load_memory_map {
            memory_map_path.map(MemoryMap::load).transpose()?
        } else {
            None
        };
        let svd_paths = if !options.svd_paths.is_empty() {
            options.svd_paths
        } else {
            project.svd_paths.clone()
        };
        let mut mmio = if options.load_register_catalog {
            crate::register_catalog::load(&svd_paths, Some(&project))?
        } else {
            MmioMap::load_all(&[])?
        };
        if let Some(memory_map) = &memory_map {
            mmio.regions.extend(memory_map.resolved_mmio_regions()?);
            mmio.regions
                .sort_by_key(|region| (region.start, region.end, region.name.clone()));
            mmio.regions.dedup();
        }

        Ok(Self {
            manifest,
            project,
            target_path,
            target,
            run_spec_path,
            run_spec,
            memory_map,
            svd_paths,
            mmio,
            explicit_context,
            invocation_directory,
            function_workspace: OnceLock::new(),
            code_workspace: OnceLock::new(),
            register_workspace: OnceLock::new(),
            interface_workspace: OnceLock::new(),
            artifacts: ProjectArtifactStore::default(),
        })
    }

    pub(crate) fn context(&self) -> ProjectContext<'_> {
        ProjectContext {
            project_path: &self.manifest,
            project: &self.project,
            target_path: &self.target_path,
            target: &self.target,
            run_spec_path: self.run_spec_path.as_deref(),
            run_spec: self.run_spec.as_ref(),
            memory_map: self.memory_map.as_ref(),
            svd_paths: &self.svd_paths,
            svd: &self.mmio,
            explicit_context: &self.explicit_context,
            invocation_directory: &self.invocation_directory,
        }
    }

    /// Canonical identity of the exact applicability context authenticated for
    /// this session. Cache signatures bind this value even when the artifact
    /// selecting a reviewed fact is not a stage-local input.
    pub(crate) fn active_applicability_identity(&self) -> String {
        serde_json::to_string(&self.project.review_context)
            .expect("validated applicability context is JSON serializable")
    }

    /// Reauthenticate active artifact bytes after a long-lived generation
    /// guard has been captured. This closes the window between opening a
    /// session and starting a pipeline over caller-owned files.
    pub(crate) fn validate_active_artifacts(&self) -> Result<()> {
        let knowledge =
            open_radio_vendor_review::ReviewKnowledge::load_all(&self.project.reviewed_knowledge)
                .map_err(|error| {
                crate::Error::invalid(format!("cannot reload reviewed knowledge: {error}"))
            })?;
        let sources = knowledge.constrained_artifact_sources();
        if sources.is_empty() {
            return Ok(());
        }
        let run_spec = self.run_spec.as_ref().ok_or_else(|| {
            crate::Error::invalid("cannot reauthenticate reviewed facts without an active run spec")
        })?;
        let current = run_spec.artifact_identities_for_sources(&sources)?;
        if current != self.project.review_context.artifacts {
            return Err(crate::Error::invalid(
                "active vendor inputs changed after reviewed-fact applicability was selected; reopen the project and rerun the command",
            ));
        }
        Ok(())
    }

    pub(crate) fn function_workspace(
        &self,
    ) -> Result<Option<&crate::function_workspace::FunctionWorkspace>> {
        cached_function_workspace(&self.project, &self.function_workspace)
    }

    pub(crate) fn code_workspace(&self) -> Result<Option<&crate::code_workspace::CodeWorkspace>> {
        cached_code_workspace(&self.project, &self.code_workspace)
    }

    pub(crate) fn register_workspace(
        &self,
    ) -> Result<Option<&crate::registers::ProjectRegisterWorkspace>> {
        cached_register_workspace(&self.project, &self.register_workspace)
    }

    pub(crate) fn interface_workspace(
        &self,
    ) -> Result<Option<&crate::interfaces::InterfaceWorkspace>> {
        cached_interface_workspace(&self.project, &self.target, &self.interface_workspace)
    }

    pub(crate) fn linked_ir(
        &self,
        path: &std::path::Path,
    ) -> Result<std::sync::Arc<crate::artifacts::LinkedIrReader>> {
        self.artifacts.linked_ir(path)
    }
}

fn cached_code_workspace<'a>(
    project: &crate::ProjectSpec,
    cache: &'a OnceLock<std::result::Result<Option<crate::code_workspace::CodeWorkspace>, String>>,
) -> Result<Option<&'a crate::code_workspace::CodeWorkspace>> {
    cached_optional(cache, || {
        let (Some(paths), Some(inventory)) = (&project.code, &project.symbol_inventory) else {
            return Ok(None);
        };
        if !inventory.output.is_file() || !paths.pack.is_file() {
            return Ok(None);
        }
        let facts =
            crate::artifacts::symbol_inventory::load_code_boundary_facts(&inventory.output)?;
        crate::code_workspace::CodeWorkspace::load(&facts, &paths.pack, &project.id).map(Some)
    })
}

fn cached_register_workspace<'a>(
    project: &crate::ProjectSpec,
    cache: &'a OnceLock<
        std::result::Result<Option<crate::registers::ProjectRegisterWorkspace>, String>,
    >,
) -> Result<Option<&'a crate::registers::ProjectRegisterWorkspace>> {
    cached_optional(cache, || {
        let Some(paths) = project.registers.as_ref() else {
            return Ok(None);
        };
        if !paths.model.is_file() {
            return Ok(None);
        }
        crate::registers::ProjectRegisterWorkspace::load(paths).map(Some)
    })
}

fn cached_interface_workspace<'a>(
    project: &crate::ProjectSpec,
    target: &crate::TargetSpec,
    cache: &'a OnceLock<std::result::Result<Option<crate::interfaces::InterfaceWorkspace>, String>>,
) -> Result<Option<&'a crate::interfaces::InterfaceWorkspace>> {
    cached_optional(cache, || {
        let Some(paths) = project.interfaces.as_ref() else {
            return Ok(None);
        };
        let Some(pack) = paths.pack.as_ref() else {
            return Ok(None);
        };
        if !paths.facts.is_file() || !pack.is_file() {
            return Ok(None);
        }
        let contracts = target
            .knowledge_provider
            .as_deref()
            .and_then(|harness| crate::harnesses::contracts(harness).ok());
        crate::interfaces::InterfaceWorkspace::load_with_templates(
            &paths.facts,
            pack,
            &paths.semantic_catalogs,
            &paths.interface_template_packs,
            target.calling_convention.label(),
            contracts,
        )
        .map(Some)
    })
}

fn cached_optional<T>(
    cache: &OnceLock<std::result::Result<Option<T>, String>>,
    load: impl FnOnce() -> Result<Option<T>>,
) -> Result<Option<&T>> {
    match cache.get_or_init(|| load().map_err(|error| error.to_string())) {
        Ok(value) => Ok(value.as_ref()),
        Err(message) => Err(crate::Error::invalid(message.clone())),
    }
}

fn cached_function_workspace<'a>(
    project: &crate::ProjectSpec,
    cache: &'a OnceLock<
        std::result::Result<Option<crate::function_workspace::FunctionWorkspace>, String>,
    >,
) -> Result<Option<&'a crate::function_workspace::FunctionWorkspace>> {
    let cached = cache.get_or_init(|| {
        let Some(paths) = project.functions.as_ref() else {
            return Ok(None);
        };
        let reports = project
            .function_ir_reports()
            .map_err(|error| error.to_string())?;
        if reports.iter().any(|(_, report)| !report.is_dir()) || !paths.pack.is_file() {
            return Ok(None);
        }
        crate::function_workspace::FunctionWorkspace::load_summary(&reports, &paths.pack)
            .map(Some)
            .map_err(|error| error.to_string())
    });
    match cached {
        Ok(workspace) => Ok(workspace.as_ref()),
        Err(message) => Err(crate::Error::invalid(message.clone())),
    }
}
