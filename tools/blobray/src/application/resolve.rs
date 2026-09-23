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

pub(crate) struct ProjectSessionInputs {
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
    function_workspace:
        OnceLock<std::result::Result<Option<crate::function_workspace::FunctionWorkspace>, String>>,
    code_workspace:
        OnceLock<std::result::Result<Option<crate::code_workspace::CodeWorkspace>, String>>,
    interface_facts: OnceLock<
        std::result::Result<Option<std::sync::Arc<crate::interfaces::InterfaceFacts>>, String>,
    >,
    interface_workspace:
        OnceLock<std::result::Result<Option<crate::interfaces::InterfaceWorkspace>, String>>,
    register_query:
        OnceLock<std::result::Result<super::register_query::RegisterQueryCapture, String>>,
    pub(crate) artifacts: ProjectArtifactStore,
}

impl ProjectSession {
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
            let mut sources = knowledge.constrained_artifact_sources();
            sources.extend(crate::providers::reviewed_memory_access_artifact_sources(
                target.knowledge_provider.as_deref(),
            )?);
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
        let svd_paths = if !options.svd_paths.is_empty() {
            options.svd_paths
        } else {
            project.svd_paths.clone()
        };
        let (memory_map, mmio) = load_analysis_models(
            &mut project,
            &svd_paths,
            options.load_memory_map,
            options.load_register_catalog,
        )?;

        Ok(Self::from_inputs(ProjectSessionInputs {
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
        }))
    }

    /// Prepare backend models from this session's resolved declarations.
    /// Never re-resolve the manifest or lose the caller's overrides.
    pub(crate) fn with_analysis_models(&self) -> Result<Self> {
        let mut project = self.project.clone();
        let (memory_map, mmio) = load_analysis_models(&mut project, &self.svd_paths, true, true)?;
        Ok(Self::from_inputs(ProjectSessionInputs {
            manifest: self.manifest.clone(),
            project,
            target_path: self.target_path.clone(),
            target: self.target.clone(),
            run_spec_path: self.run_spec_path.clone(),
            run_spec: self.run_spec.clone(),
            memory_map,
            svd_paths: self.svd_paths.clone(),
            mmio,
            explicit_context: self.explicit_context.clone(),
            invocation_directory: self.invocation_directory.clone(),
        }))
    }

    /// Start a session with fresh owner-managed caches for resolved inputs.
    pub(crate) fn from_inputs(inputs: ProjectSessionInputs) -> Self {
        let artifacts = ProjectArtifactStore::new(&inputs.manifest, &inputs.project);
        Self {
            manifest: inputs.manifest,
            project: inputs.project,
            target_path: inputs.target_path,
            target: inputs.target,
            run_spec_path: inputs.run_spec_path,
            run_spec: inputs.run_spec,
            memory_map: inputs.memory_map,
            svd_paths: inputs.svd_paths,
            mmio: inputs.mmio,
            explicit_context: inputs.explicit_context,
            invocation_directory: inputs.invocation_directory,
            function_workspace: OnceLock::new(),
            code_workspace: OnceLock::new(),
            interface_workspace: OnceLock::new(),
            interface_facts: OnceLock::new(),
            register_query: OnceLock::new(),
            artifacts,
        }
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
        let mut sources = knowledge.constrained_artifact_sources();
        sources.extend(crate::providers::reviewed_memory_access_artifact_sources(
            self.target.knowledge_provider.as_deref(),
        )?);
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
        cached_function_workspace(self)
    }

    pub(crate) fn code_workspace(&self) -> Result<Option<&crate::code_workspace::CodeWorkspace>> {
        cached_code_workspace(self)
    }

    pub(crate) fn register_detail(
        &self,
        selector: &super::RegisterSelector,
    ) -> Result<Option<super::RegisterDetailSummary>> {
        super::snapshot::registers::detail(self, selector)
    }

    pub(crate) fn register_query(&self) -> Result<&super::register_query::RegisterQueryCapture> {
        self.register_query
            .get_or_init(|| {
                super::register_query::RegisterQueryCapture::capture(self)
                    .map_err(|error| error.to_string())
            })
            .as_ref()
            .map_err(|reason| crate::Error::invalid(reason.clone()))
    }

    pub(crate) fn interface_facts(
        &self,
    ) -> Result<Option<&std::sync::Arc<crate::interfaces::InterfaceFacts>>> {
        cached_optional(&self.interface_facts, || {
            let Some(paths) = &self.project.interfaces else {
                return Ok(None);
            };
            let input = self.artifacts.read_text(&paths.facts)?;
            crate::interfaces::InterfaceFacts::parse(&paths.facts, &input)
                .map(|facts| Some(std::sync::Arc::new(facts)))
        })
    }

    pub(crate) fn interface_workspace(
        &self,
    ) -> Result<Option<&crate::interfaces::InterfaceWorkspace>> {
        cached_interface_workspace(
            &self.project,
            &self.target,
            &self.interface_workspace,
            self.interface_facts()?.cloned(),
        )
    }

    pub(crate) fn linked_ir(
        &self,
        path: &std::path::Path,
    ) -> Result<std::sync::Arc<crate::artifacts::LinkedIrReader>> {
        self.artifacts.linked_ir(path)
    }
}

fn cached_code_workspace(
    session: &ProjectSession,
) -> Result<Option<&crate::code_workspace::CodeWorkspace>> {
    cached_optional(&session.code_workspace, || {
        let project = &session.project;
        let (Some(paths), Some(inventory)) = (&project.code, &project.symbol_inventory) else {
            return Ok(None);
        };
        if !paths.pack.is_file() {
            return Ok(None);
        }
        let input = session.artifacts.read_text(&inventory.output)?;
        let facts = crate::artifacts::symbol_inventory::parse_code_boundary_facts(
            &inventory.output,
            &input,
        )?;
        crate::code_workspace::CodeWorkspace::load(&facts, &paths.pack, &project.id).map(Some)
    })
}

fn cached_interface_workspace<'a>(
    project: &crate::ProjectSpec,
    target: &crate::TargetSpec,
    cache: &'a OnceLock<std::result::Result<Option<crate::interfaces::InterfaceWorkspace>, String>>,
    facts: Option<std::sync::Arc<crate::interfaces::InterfaceFacts>>,
) -> Result<Option<&'a crate::interfaces::InterfaceWorkspace>> {
    cached_optional(cache, || {
        let Some(paths) = project.interfaces.as_ref() else {
            return Ok(None);
        };
        let Some(pack) = paths.pack.as_ref() else {
            return Ok(None);
        };
        let Some(facts) = facts else {
            return Ok(None);
        };
        if !pack.is_file() {
            return Ok(None);
        }
        let contracts = target
            .knowledge_provider
            .as_deref()
            .map(crate::providers::contracts)
            .transpose()?;
        crate::interfaces::InterfaceWorkspace::from_facts(
            facts,
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

fn cached_function_workspace(
    session: &ProjectSession,
) -> Result<Option<&crate::function_workspace::FunctionWorkspace>> {
    cached_optional(&session.function_workspace, || {
        let Some(paths) = session.project.functions.as_ref() else {
            return Ok(None);
        };
        if !paths.pack.is_file() {
            return Ok(None);
        }
        let reports = session.project.function_ir_reports()?;
        let facts =
            crate::function_workspace::FunctionFacts::load_summary_with(&reports, |path| {
                session.linked_ir(path)?.read_review_projection()
            })?;
        crate::function_workspace::FunctionWorkspace::from_summary_facts(facts, &paths.pack)
            .map(Some)
    })
}

/// Backend model preparation is explicit; inventory queries read the complete
/// source geometry independently of the backend's address/width limits.
fn load_analysis_models(
    project: &mut ProjectSpec,
    svd_paths: &[PathBuf],
    load_memory_map: bool,
    load_register_catalog: bool,
) -> Result<(Option<MemoryMap>, MmioMap)> {
    let memory_map_path = project.memory_map.as_deref();
    let memory_map = if load_memory_map {
        memory_map_path
            .map(|path| {
                use sha2::{Digest, Sha256};
                let mut copies = crate::verification::ExecutionInputs::new()?;
                let mut copy = path.to_owned();
                copies.capture(&mut copy)?;
                let model = MemoryMap::load(&copy)?;
                project.loaded_model_inputs.insert(
                    path.canonicalize()?,
                    format!("{:x}", Sha256::digest(std::fs::read(&copy)?)),
                );
                Ok::<_, crate::Error>(model)
            })
            .transpose()?
    } else {
        None
    };
    let mut mmio = if load_register_catalog {
        let (catalog, inputs) =
            crate::register_catalog::load_with_inputs(svd_paths, Some(project))?;
        project.loaded_model_inputs.extend(inputs);
        catalog
    } else {
        MmioMap::load_all(&[])?
    };
    if load_register_catalog && let Some(memory_map) = &memory_map {
        mmio.regions.extend(memory_map.resolved_mmio_regions()?);
        mmio.regions
            .sort_by_key(|region| (region.start, region.end, region.name.clone()));
        mmio.regions.dedup();
    }
    Ok((memory_map, mmio))
}
