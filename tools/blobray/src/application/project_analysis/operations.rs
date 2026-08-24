//! Frontend-neutral project analysis/review operations over domain workspaces.

use crate::{
    MemoryMap, MmioMap, Result,
    analysis::{
        DiscoveryRange, EffectiveCodeCatalog, ProjectInterfaceDiscoveryOptions,
        build_project_linkage_inventory, discover_mmio, discover_project_interfaces,
    },
    artifact,
    artifacts::{
        build_interface_facts, build_mmio_facts as mmio_document, build_symbol_inventory_document,
    },
    code_workspace::{CodeWorkspace, render_code_boundary_review},
    function_workspace::{FunctionWorkspace, link_reviewed_interfaces, render_function_review},
    interfaces::InterfaceWorkspace,
    project::ProjectSpec,
    registers::{
        ProjectRegisterWorkspace, RegisterFacts, RegisterModel, load_effective_register_model,
        render_register_review, validate_pac_api, validate_register_evidence,
        validate_register_lints, validate_register_memory_map,
    },
    run_spec::{InputRole, RunSpec},
};

use super::{
    ProjectAnalysisInputs, ProjectAnalysisOperations, ProjectAnalysisPlanAction,
    ProjectAnalysisPlanReport, ProjectAnalysisPlanWorkItem, ProjectAnalysisPlanner,
    ProjectAnalysisReport, ProjectAnalysisRequest,
    cache::{PipelineInputObservation, ProjectAnalysisCache, ProjectAnalysisCachePlan},
};
use crate::application::{ProjectSession, pipeline::StageRun};

pub(crate) fn analyze_project(
    session: &ProjectSession,
    request: ProjectAnalysisRequest,
) -> ProjectAnalysisReport {
    let inputs = project_analysis_inputs(session);
    let compiled_knowledge_identity =
        crate::harnesses::analysis_cache_identity(session.target.knowledge_provider.as_deref());
    let (pipeline_inputs, pipeline_input_error) = pipeline_input_observation(session);
    let mut operations = ResolvedProjectAnalysisOperations {
        session,
        cache: if request.check {
            ProjectAnalysisCache::disabled()
        } else {
            ProjectAnalysisCache::deferred(&session.manifest)
        }
        .with_compiled_knowledge_identity(compiled_knowledge_identity),
        check: request.check,
        functions: None,
        interfaces: None,
        planner: None,
        pipeline_inputs,
        pipeline_input_error,
    };
    super::run(&session.project, request, inputs, &mut operations)
}

pub(crate) fn plan_project(
    session: &ProjectSession,
    request: ProjectAnalysisRequest,
) -> ProjectAnalysisPlanReport {
    let inputs = project_analysis_inputs(session);
    let compiled_knowledge_identity =
        crate::harnesses::analysis_cache_identity(session.target.knowledge_provider.as_deref());
    let (pipeline_inputs, pipeline_input_error) = pipeline_input_observation(session);
    let mut operations = ResolvedProjectAnalysisOperations {
        session,
        cache: ProjectAnalysisCache::planning(&session.manifest)
            .with_compiled_knowledge_identity(compiled_knowledge_identity),
        check: request.check,
        functions: None,
        interfaces: None,
        planner: Some(ProjectAnalysisPlanner::default()),
        pipeline_inputs,
        pipeline_input_error,
    };
    let execution = super::run(&session.project, request, inputs, &mut operations);
    operations
        .planner
        .take()
        .expect("project analysis planner was configured")
        .finish(&session.project, inputs, execution)
}

fn project_analysis_inputs(session: &ProjectSession) -> ProjectAnalysisInputs {
    let replay_requirements = match session.project.functions.as_ref() {
        None => (false, false),
        Some(paths) => match crate::function_workspace::FunctionPack::load_reviewed(&paths.pack) {
            Ok(pack) => {
                let replays = pack
                    .event_routes
                    .iter()
                    .filter_map(|route| route.replay.as_ref())
                    .collect::<Vec<_>>();
                let requires_interfaces = replays.iter().any(|replay| {
                    crate::application::event_replay::manifest_requires_reviewed_interfaces(
                        &replay.manifest,
                    )
                    .unwrap_or(true)
                });
                (!replays.is_empty(), requires_interfaces)
            }
            // Preserve fail-closed orchestration for an invalid reviewed pack:
            // the event stage will surface the precise load error when it runs.
            Err(_) => (true, true),
        },
    };
    ProjectAnalysisInputs {
        run_spec: session.run_spec.is_some(),
        memory_map: session.memory_map.is_some(),
        event_replays: replay_requirements.0,
        event_replays_require_interfaces: replay_requirements.1,
    }
}

fn pipeline_input_observation(
    session: &ProjectSession,
) -> (Option<PipelineInputObservation>, Option<String>) {
    match pipeline_input_paths(session).and_then(PipelineInputObservation::capture) {
        Ok(observation) => (Some(observation), None),
        Err(error) => (None, Some(error.to_string())),
    }
}

fn pipeline_input_paths(session: &ProjectSession) -> Result<Vec<std::path::PathBuf>> {
    let mut paths = crate::application::project_files::collect(&session.context())?
        .files
        .into_iter()
        .filter(|file| {
            file.ownership != crate::application::project_files::ProjectFileOwnership::Generated
        })
        .map(|file| file.path)
        .collect::<Vec<_>>();

    if let Some(registers) = session
        .project
        .registers
        .as_ref()
        .filter(|registers| registers.model.is_file())
    {
        paths.extend(RegisterModel::input_paths(&registers.model)?);
    }
    if let Some(functions) = session
        .project
        .functions
        .as_ref()
        .filter(|functions| functions.pack.is_file())
    {
        let pack = crate::function_workspace::FunctionPack::load_reviewed(&functions.pack)?;
        paths.extend(
            pack.event_routes
                .iter()
                .filter_map(|route| route.replay.as_ref())
                .map(|replay| replay.manifest.clone()),
        );
    }
    Ok(paths)
}

struct ResolvedProjectAnalysisOperations<'a> {
    session: &'a ProjectSession,
    cache: ProjectAnalysisCache,
    check: bool,
    functions: Option<FunctionWorkspace>,
    interfaces: Option<InterfaceWorkspace>,
    planner: Option<ProjectAnalysisPlanner>,
    pipeline_inputs: Option<PipelineInputObservation>,
    pipeline_input_error: Option<String>,
}

fn register_catalog_input_paths(
    svd_paths: &[std::path::PathBuf],
    registers: Option<&crate::project::RegisterWorkspacePaths>,
) -> Result<Vec<std::path::PathBuf>> {
    let mut paths = svd_paths.to_vec();
    if let Some(registers) = registers {
        paths.extend(RegisterModel::input_paths(&registers.model)?);
        paths.extend(registers.reviewed_knowledge.iter().cloned());
    }
    Ok(paths)
}

impl ResolvedProjectAnalysisOperations<'_> {
    fn plan_stage(
        &mut self,
        stage: &str,
        cache_stage: &str,
        check: bool,
        inputs: &[std::path::PathBuf],
        outputs: &[std::path::PathBuf],
    ) -> Result<Option<StageRun>> {
        let Some(planner) = self.planner.as_ref() else {
            return Ok(None);
        };
        self.cache.ensure_planning_snapshot()?;
        let materializations = inputs
            .iter()
            .filter_map(|input| {
                planner
                    .input_materialization(input)
                    .map(|(dependency, output)| {
                        (input.clone(), dependency.to_owned(), output.to_owned())
                    })
            })
            .collect::<Vec<_>>();
        let concrete_inputs = inputs
            .iter()
            .filter(|input| {
                !materializations
                    .iter()
                    .any(|(materialized, _, _)| materialized == *input)
            })
            .cloned()
            .collect::<Vec<_>>();
        // A generated predecessor can defer only the paths that it owns. All
        // independent inputs must still pass preflight now, otherwise a plan
        // can claim READY even though execution will fail before this stage.
        self.ensure_cache_inputs(cache_stage, &concrete_inputs)?;
        if materializations.is_empty() && check {
            ensure_check_outputs(outputs)?;
        }
        let awaiting_inputs = materializations
            .iter()
            .map(
                |(input, dependency, _)| super::ProjectAnalysisPlanAwaitingInput {
                    path: input.clone(),
                    producer_stage: dependency.clone(),
                },
            )
            .collect::<Vec<_>>();
        let (action, signature, cause) = if !materializations.is_empty() {
            let producers = materializations
                .iter()
                .map(|(_, dependency, _)| dependency.as_str())
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .collect::<Vec<_>>()
                .join(", ");
            (
                ProjectAnalysisPlanAction::Deferred,
                None,
                Some(format!(
                    "cache state awaits {} generated input(s) from stage(s) {producers}",
                    materializations.len()
                )),
            )
        } else if check {
            (
                ProjectAnalysisPlanAction::Verify,
                None,
                Some(
                    "check mode executes the stage and compares its outputs without writing"
                        .to_owned(),
                ),
            )
        } else if self.is_unversioned_linked_ir_stage(cache_stage) {
            (
                ProjectAnalysisPlanAction::Compute,
                None,
                Some(
                    "persistent cache is disabled because the selected harness has no stable semantic cache domain"
                        .to_owned(),
                ),
            )
        } else {
            let configuration = self.stage_configuration(cache_stage);
            match self
                .cache
                .plan(cache_stage, &configuration, inputs, outputs)?
            {
                ProjectAnalysisCachePlan::Current { signature } => {
                    (ProjectAnalysisPlanAction::Current, Some(signature), None)
                }
                ProjectAnalysisCachePlan::Restorable {
                    signature,
                    changed_outputs,
                } => (
                    ProjectAnalysisPlanAction::Restore,
                    Some(signature),
                    Some(format!(
                        "{changed_outputs} generated output(s) are missing or differ from the matching cached result"
                    )),
                ),
                ProjectAnalysisCachePlan::Missing { signature, cause } => (
                    ProjectAnalysisPlanAction::Compute,
                    Some(signature),
                    Some(cause),
                ),
            }
        };
        self.planner
            .as_mut()
            .expect("planner presence was checked")
            .record(
                stage,
                ProjectAnalysisPlanWorkItem {
                    name: cache_stage.to_owned(),
                    action,
                    signature,
                    outputs: outputs.to_vec(),
                    cause,
                    awaiting_inputs,
                },
            );
        Ok(Some(if action == ProjectAnalysisPlanAction::Current {
            StageRun::Current
        } else {
            StageRun::Executed
        }))
    }

    fn run_spec(&self) -> Result<&RunSpec> {
        self.session
            .run_spec
            .as_ref()
            .ok_or_else(|| crate::Error::invalid("run-spec is not configured"))
    }

    fn memory_map(&self) -> Result<&MemoryMap> {
        self.session
            .memory_map
            .as_ref()
            .ok_or_else(|| crate::Error::invalid("memory-map is not configured"))
    }

    fn linked_ir_inputs(
        &self,
        profile: &crate::project_ir::ProjectIrProfile,
    ) -> Result<Vec<std::path::PathBuf>> {
        let project = &self.session.project;
        let mut paths = vec![self.session.target_path.clone()];
        paths.extend(self.session.run_spec_path.iter().cloned());
        paths.extend(self.register_catalog_inputs()?);
        for pack in &project.ecosystem_packs {
            paths.push(pack.path.clone());
            paths.extend(pack.knowledge_packs.iter().cloned());
        }
        if let Some(pack) = project.chip_pack.as_ref() {
            paths.push(pack.path.clone());
            paths.extend(pack.knowledge_packs.iter().cloned());
        }
        paths.extend(crate::application::project_ir_build::profile_input_paths(
            profile,
            self.run_spec()?,
        )?);
        if let Some(code) = project.code.as_ref() {
            paths.push(code.pack.clone());
        }
        if let Some(interfaces) = project
            .interfaces
            .as_ref()
            .filter(|_| super::linked_ir_uses_reviewed_interfaces(project))
        {
            paths.push(interfaces.facts.clone());
            paths.extend(interfaces.pack.iter().cloned());
            paths.extend(interfaces.semantic_catalogs.iter().cloned());
        }
        if let Some(symbols) = project.symbol_inventory.as_ref() {
            paths.push(symbols.output.clone());
        }
        Ok(paths)
    }

    /// Every source merged into the resolved MMIO catalog. Register models
    /// are optional overlays, but when a v2 model exists its fragments are
    /// semantic inputs just like its top-level manifest.
    fn register_catalog_inputs(&self) -> Result<Vec<std::path::PathBuf>> {
        let mut paths = register_catalog_input_paths(
            &self.session.svd_paths,
            self.session.project.registers.as_ref(),
        )?;
        if self.session.memory_map.is_some() {
            paths.extend(self.session.project.memory_map.iter().cloned());
        }
        Ok(paths)
    }

    fn common_inputs(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    fn run_inputs(&self) -> Vec<std::path::PathBuf> {
        self.run_inputs_matching(|_| true)
    }

    fn mmio_run_inputs(&self) -> Vec<std::path::PathBuf> {
        self.run_inputs_matching(|role| matches!(role, InputRole::SourceArtifact(_)))
    }

    fn interface_discovery_run_inputs(&self) -> Vec<std::path::PathBuf> {
        self.run_inputs_matching(InputRole::is_scannable)
    }

    fn run_inputs_matching(&self, include: impl Fn(&InputRole) -> bool) -> Vec<std::path::PathBuf> {
        let mut paths = self.common_inputs();
        paths.extend(self.session.run_spec_path.iter().cloned());
        if let Some(run_spec) = self.session.run_spec.as_ref() {
            paths.extend(
                run_spec
                    .inputs()
                    .iter()
                    .filter(|input| include(&input.role))
                    .map(|input| input.path.clone()),
            );
        }
        paths
    }

    fn target_inputs(&self) -> Vec<std::path::PathBuf> {
        let mut paths = vec![self.session.target_path.clone()];
        for pack in &self.session.project.ecosystem_packs {
            paths.push(pack.path.clone());
            paths.extend(pack.knowledge_packs.iter().cloned());
        }
        if let Some(pack) = self.session.project.chip_pack.as_ref() {
            paths.push(pack.path.clone());
            paths.extend(pack.knowledge_packs.iter().cloned());
        }
        paths
    }

    fn interface_workspace_inputs(&self) -> Vec<std::path::PathBuf> {
        let mut paths = self.target_inputs();
        if let Some(interfaces) = self.session.project.interfaces.as_ref() {
            paths.push(interfaces.facts.clone());
            paths.extend(interfaces.pack.iter().cloned());
            paths.extend(interfaces.semantic_catalogs.iter().cloned());
        }
        paths
    }

    fn reviewed_interface_workspace_inputs(&self) -> Vec<std::path::PathBuf> {
        if self.has_reviewed_interface_workspace() {
            self.interface_workspace_inputs()
        } else {
            Vec::new()
        }
    }

    fn has_reviewed_interface_workspace(&self) -> bool {
        self.session
            .project
            .interfaces
            .as_ref()
            .and_then(|paths| paths.pack.as_deref())
            .is_some_and(std::path::Path::is_file)
    }

    fn ensure_cache_inputs(&self, stage: &str, inputs: &[std::path::PathBuf]) -> Result<()> {
        for input in inputs {
            let optional_missing_symbol_projection = (stage == "linked-ir"
                || stage.starts_with("linked-ir:"))
                && self.session.project.code.is_none()
                && self
                    .session
                    .project
                    .symbol_inventory
                    .as_ref()
                    .is_some_and(|symbols| symbols.output == *input)
                && !input.exists();
            if !optional_missing_symbol_projection {
                ensure_stage_input(input)?;
            }
        }
        Ok(())
    }

    fn function_workspace_inputs(&self) -> Result<Vec<std::path::PathBuf>> {
        let mut paths = self.common_inputs();
        if let Some(functions) = self.session.project.functions.as_ref() {
            paths.push(functions.pack.clone());
        }
        paths.extend(
            self.session
                .project
                .function_ir_reports()?
                .into_iter()
                .map(|(_, path)| path),
        );
        Ok(paths)
    }

    fn ensure_function_workspace(&mut self) -> Result<()> {
        if self.functions.is_none() {
            let paths = self
                .session
                .project
                .functions
                .as_ref()
                .ok_or_else(|| crate::Error::invalid("[functions] is absent"))?;
            let reports = self.session.project.function_ir_reports()?;
            self.functions = Some(FunctionWorkspace::load(&reports, &paths.pack)?);
        }
        Ok(())
    }

    fn ensure_interface_workspace(&mut self) -> Result<()> {
        if self.interfaces.is_none() {
            let paths = self
                .session
                .project
                .interfaces
                .as_ref()
                .ok_or_else(|| crate::Error::invalid("[interfaces] is absent"))?;
            let pack = paths
                .pack
                .as_deref()
                .ok_or_else(|| crate::Error::invalid("[interfaces].pack is absent"))?;
            self.interfaces = Some(InterfaceWorkspace::load(
                &paths.facts,
                pack,
                &paths.semantic_catalogs,
                self.session.target.calling_convention.label(),
                self.session
                    .target
                    .knowledge_provider
                    .as_deref()
                    .map(crate::harnesses::contracts)
                    .transpose()?,
            )?);
        }
        Ok(())
    }

    fn register_workspace_inputs(&self, include_ir: bool) -> Result<Vec<std::path::PathBuf>> {
        let mut paths = self.common_inputs();
        paths.extend(self.session.project.memory_map.iter().cloned());
        if let Some(registers) = self.session.project.registers.as_ref() {
            paths.push(registers.facts.clone());
            paths.extend(RegisterModel::input_paths(&registers.model)?);
            paths.extend(registers.reviewed_knowledge.iter().cloned());
            paths.extend(registers.api_pack.iter().cloned());
            paths.extend(registers.lint_pack.iter().cloned());
            paths.extend(registers.evidence_catalogs.iter().cloned());
            if include_ir {
                paths.extend(registers.review_ir_reports.iter().cloned());
            }
        }
        Ok(paths)
    }

    fn cache_hit(
        &mut self,
        stage: &str,
        check: bool,
        inputs: &[std::path::PathBuf],
        outputs: &[std::path::PathBuf],
    ) -> Result<Option<StageRun>> {
        self.ensure_cache_inputs(stage, inputs)?;
        if check {
            return Ok(None);
        }
        if self.is_unversioned_linked_ir_stage(stage) {
            tracing::warn!(
                cache_stage = stage,
                "persistent linked-IR stage cache disabled: the selected RISC-V harness has no stable semantic cache domain"
            );
            return Ok(None);
        }
        let configuration = self.stage_configuration(stage);
        let current = self
            .cache
            .is_current(stage, &configuration, inputs, outputs)?;
        if current {
            let run = if self.cache.last_lookup_restored() {
                tracing::info!(cache_stage = stage, "restored content-addressed outputs");
                StageRun::Restored
            } else {
                tracing::info!(cache_stage = stage, "content-addressed outputs are current");
                StageRun::Current
            };
            Ok(Some(run))
        } else {
            Ok(None)
        }
    }

    fn cache_record(
        &mut self,
        stage: &str,
        check: bool,
        inputs: &[std::path::PathBuf],
        outputs: &[std::path::PathBuf],
    ) -> Result<()> {
        if !check && !self.is_unversioned_linked_ir_stage(stage) {
            let configuration = self.stage_configuration(stage);
            self.cache.record(stage, &configuration, inputs, outputs)?;
        }
        Ok(())
    }

    fn linked_ir_semantic_cache_domain(&self) -> Option<&'static str> {
        crate::harnesses::riscv_or_neutral(self.session.target.knowledge_provider.as_deref())
            .ok()
            .map(|harness| harness.semantic_cache_domain)
    }

    fn is_unversioned_linked_ir_stage(&self, stage: &str) -> bool {
        !linked_ir_stage_cacheable(stage, self.linked_ir_semantic_cache_domain())
    }

    /// Stable, stage-owned project configuration included in the cache key.
    ///
    /// File contents remain explicit inputs. Keeping unrelated manifest
    /// sections out of this value prevents, for example, a review-scope edit
    /// from invalidating artifact-wide decoding and linked IR.
    fn stage_configuration(&self, stage: &str) -> String {
        stage_configuration(
            &self.session.project,
            stage,
            self.linked_ir_semantic_cache_domain(),
        )
    }

    fn linked_ir_outputs(
        &self,
        profile: &crate::project_ir::ProjectIrProfile,
    ) -> Vec<std::path::PathBuf> {
        crate::artifacts::bundle_files(&profile.output).collect()
    }

    fn all_linked_ir_outputs(&self) -> Vec<std::path::PathBuf> {
        self.session
            .project
            .ir_profiles
            .iter()
            .flat_map(|profile| self.linked_ir_outputs(profile))
            .collect()
    }
}

fn stage_configuration(
    project: &ProjectSpec,
    stage: &str,
    linked_ir_semantic_cache_domain: Option<&str>,
) -> String {
    if let Some(profile_id) = stage.strip_prefix("linked-ir:") {
        let profile = project
            .ir_profiles
            .iter()
            .find(|profile| profile.id == profile_id)
            .expect("linked-IR cache stage must name a configured profile");
        return format!(
            "sources={:?};roots={:?};include-reachable={};entry-contract={:?};effective-code-domain={:?};riscv-semantic-cache-domain={:?}",
            profile.sources,
            profile.roots,
            profile.include_reachable,
            profile.entry_contract,
            effective_code_domain(project),
            linked_ir_semantic_cache_domain,
        );
    }
    match stage.split_once(':').map_or(stage, |(owner, _)| owner) {
        "symbol-inventory" => format!("{:?}", project.symbol_inventory),
        "mmio-discovery" => format!(
            "facts={:?};effective-code-domain={:?}",
            project.registers.as_ref().map(|registers| &registers.facts),
            effective_code_domain(project),
        ),
        "interface-discovery" => format!(
            "facts={:?};effective-code-domain={:?}",
            project
                .interfaces
                .as_ref()
                .map(|interfaces| &interfaces.facts),
            effective_code_domain(project),
        ),
        "linked-ir" => format!(
            "profiles={:?};effective-code-domain={:?};riscv-semantic-cache-domain={:?}",
            project.ir_profiles,
            effective_code_domain(project),
            linked_ir_semantic_cache_domain,
        ),
        "event-replays" => format!("{:?}", project.functions),
        "review-scopes" => format!(
            "project-id={:?};review={:?};profile-bindings={:?};policy={:?}",
            project.id,
            project.review,
            review_profile_bindings(project),
            project
                .verification
                .as_ref()
                .and_then(|verification| verification.policy.as_ref())
        ),
        "navigation-index" => {
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
        "code-boundary-validation" | "code-boundary-review" => {
            format!("project-id={:?};code={:?}", project.id, project.code)
        }
        "register-validation" | "register-review" => {
            format!("{:?}", project.registers)
        }
        "function-validation" | "function-review" => format!(
            "functions={:?};profile-bindings={:?}",
            project.functions,
            function_profile_bindings(project)
        ),
        "interface-validation" => format!("{:?}", project.interfaces),
        _ => unreachable!("cache stage revision rejects unknown stage {stage:?}"),
    }
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

fn ensure_unique_replay_outputs<'a>(
    replays: impl IntoIterator<Item = &'a crate::function_workspace::ReviewedEventReplay>,
) -> Result<()> {
    let mut owners = std::collections::BTreeMap::new();
    for replay in replays {
        let identity = (replay.manifest.clone(), replay.source.clone());
        if let Some(previous) = owners.insert(replay.evidence.clone(), identity.clone())
            && previous != identity
        {
            return Err(crate::Error::invalid(format!(
                "event replay evidence output {} is assigned to both manifest/source {:?} and {:?}",
                replay.evidence.display(),
                previous,
                identity
            )));
        }
    }
    Ok(())
}

impl ProjectAnalysisOperations for ResolvedProjectAnalysisOperations<'_> {
    fn validate_pipeline_inputs(&mut self) -> Result<()> {
        if let Some(error) = self.pipeline_input_error.as_deref() {
            return Err(crate::Error::invalid(format!(
                "project input generation could not be captured: {error}"
            )));
        }
        self.pipeline_inputs
            .as_mut()
            .expect("pipeline input observation exists without a capture error")
            .validate()
    }

    fn symbol_inventory(&mut self, check: bool) -> Result<StageRun> {
        let inputs = self.run_inputs();
        let outputs = vec![
            self.session
                .project
                .symbol_inventory
                .as_ref()
                .expect("configured stage")
                .output
                .clone(),
        ];
        if let Some(run) = self.plan_stage(
            "symbol-inventory",
            "symbol-inventory",
            check,
            &inputs,
            &outputs,
        )? {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit("symbol-inventory", check, &inputs, &outputs)? {
            return Ok(run);
        }
        build_symbol_inventory(&self.session.project, self.run_spec()?, check)?;
        self.cache_record("symbol-inventory", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn discover_mmio(&mut self, check: bool, jobs: usize) -> Result<StageRun> {
        let mut inputs = self.mmio_run_inputs();
        inputs.extend(self.register_catalog_inputs()?);
        if let Some(code) = self.session.project.code.as_ref() {
            inputs.push(code.pack.clone());
            inputs.extend(
                self.session
                    .project
                    .symbol_inventory
                    .iter()
                    .map(|symbols| symbols.output.clone()),
            );
        }
        let outputs = vec![
            self.session
                .project
                .registers
                .as_ref()
                .expect("configured stage")
                .facts
                .clone(),
        ];
        if let Some(run) =
            self.plan_stage("mmio-discovery", "mmio-discovery", check, &inputs, &outputs)?
        {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit("mmio-discovery", check, &inputs, &outputs)? {
            return Ok(run);
        }
        discover_project_mmio(
            &self.session.project,
            self.run_spec()?,
            self.memory_map()?,
            &self.session.mmio,
            check,
            jobs,
        )?;
        self.cache_record("mmio-discovery", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn discover_interfaces(&mut self, check: bool) -> Result<StageRun> {
        let mut inputs = self.interface_discovery_run_inputs();
        if let Some(code) = self.session.project.code.as_ref() {
            inputs.push(code.pack.clone());
            inputs.extend(
                self.session
                    .project
                    .symbol_inventory
                    .iter()
                    .map(|symbols| symbols.output.clone()),
            );
        }
        let outputs = vec![
            self.session
                .project
                .interfaces
                .as_ref()
                .expect("configured stage")
                .facts
                .clone(),
        ];
        if let Some(run) = self.plan_stage(
            "interface-discovery",
            "interface-discovery",
            check,
            &inputs,
            &outputs,
        )? {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit("interface-discovery", check, &inputs, &outputs)? {
            return Ok(run);
        }
        discover_project_interfaces_operation(&self.session.project, self.run_spec()?, check)?;
        self.cache_record("interface-discovery", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn build_linked_ir(&mut self, check: bool, jobs: usize) -> Result<StageRun> {
        if self.planner.is_some() {
            let profiles = self.session.project.ir_profiles.clone();
            let mut all_current = true;
            for profile in profiles {
                let cache_stage = format!("linked-ir:{}", profile.id);
                let inputs = self.linked_ir_inputs(&profile)?;
                let outputs = self.linked_ir_outputs(&profile);
                let run = self
                    .plan_stage("linked-ir", &cache_stage, check, &inputs, &outputs)?
                    .expect("linked-IR planner is configured");
                all_current &= run == StageRun::Current;
            }
            return Ok(if all_current {
                StageRun::Current
            } else {
                StageRun::Executed
            });
        }
        if check {
            crate::application::project_ir_build::build_project_ir(
                crate::application::project_ir_build::ProjectIrBuildRequest {
                    profiles: Default::default(),
                    check: true,
                    jobs,
                    refresh_review_scopes: false,
                },
                &self.session.manifest,
                &self.session.project,
                self.run_spec()?,
                &self.session.mmio,
                &self.session.target,
            )?;
            return Ok(StageRun::Executed);
        }

        // Resolve cache state for every profile before entering the expensive
        // linker/analysis pipeline.  Building one stale profile at a time
        // reloads the same code catalog and reviewed interface knowledge for
        // each profile and prevents the analysis layer from sharing lazy
        // function queries across the selected set.
        let profiles = self.session.project.ir_profiles.clone();
        let mut stale = Vec::new();
        let mut restored = false;
        for profile in profiles {
            let stage = format!("linked-ir:{}", profile.id);
            let inputs = self.linked_ir_inputs(&profile)?;
            let outputs = self.linked_ir_outputs(&profile);
            if let Some(run) = self.cache_hit(&stage, false, &inputs, &outputs)? {
                restored |= run == StageRun::Restored;
                continue;
            }
            stale.push((profile.id, stage, inputs, outputs));
        }
        if stale.is_empty() {
            return Ok(if restored {
                StageRun::Restored
            } else {
                StageRun::Current
            });
        }

        let session = self.session;
        let function_fact_store = self.cache.query_store_mut()?;
        crate::application::project_ir_build::build_project_ir_with_store(
            crate::application::project_ir_build::ProjectIrBuildRequest {
                profiles: stale
                    .iter()
                    .map(|(profile, _, _, _)| profile.clone())
                    .collect(),
                check: false,
                jobs,
                refresh_review_scopes: false,
            },
            &session.project,
            session
                .run_spec
                .as_ref()
                .ok_or_else(|| crate::Error::invalid("run-spec is not configured"))?,
            &session.mmio,
            &session.target,
            function_fact_store,
        )?;
        for (_, stage, inputs, outputs) in stale {
            self.cache_record(&stage, false, &inputs, &outputs)?;
        }
        Ok(StageRun::Executed)
    }

    fn build_event_replays(&mut self, check: bool) -> Result<StageRun> {
        let functions = self
            .session
            .project
            .functions
            .as_ref()
            .ok_or_else(|| crate::Error::invalid("[functions] is absent"))?;
        let pack = crate::function_workspace::FunctionPack::load_reviewed(&functions.pack)?;
        let run_spec = self.run_spec()?;
        ensure_unique_replay_outputs(
            pack.event_routes
                .iter()
                .filter_map(|route| route.replay.as_ref()),
        )?;
        let mut configured = std::collections::BTreeMap::new();
        for replay in pack
            .event_routes
            .iter()
            .filter_map(|route| route.replay.as_ref())
        {
            configured
                .entry((
                    replay.manifest.clone(),
                    replay.source.clone(),
                    replay.evidence.clone(),
                ))
                .or_insert_with(|| replay.clone());
        }
        if configured.is_empty() {
            return Err(crate::Error::invalid(
                "event-replays stage has no configured reviewed replay",
            ));
        }

        let mut requests = Vec::with_capacity(configured.len());
        for replay in configured.into_values() {
            let artifact = run_spec
                .inputs()
                .iter()
                .find_map(|input| match &input.role {
                    InputRole::SourceArtifact(source) if source.as_str() == replay.source => {
                        Some(input.path.clone())
                    }
                    _ => None,
                })
                .ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "event replay source {:?} has no source-artifact binding in the run spec",
                        replay.source
                    ))
                })?;
            let companion = run_spec
                .inputs()
                .iter()
                .find_map(|input| match &input.role {
                    InputRole::SourceCompanion(source) if source.as_str() == replay.source => {
                        Some(input.path.clone())
                    }
                    _ => None,
                });
            requests.push((
                crate::application::event_replay::prepare(
                    crate::application::event_replay::EventReplayRequest {
                        manifest: replay.manifest,
                        artifact,
                        companion,
                    },
                )?,
                replay.evidence,
            ));
        }

        let mut inputs = self.target_inputs();
        // Replay semantics include the source-to-artifact association, not
        // merely the unordered set of artifact paths below. The run spec is
        // the caller-owned document that records that binding.
        inputs.extend(self.session.run_spec_path.iter().cloned());
        inputs.push(functions.pack.clone());
        inputs.extend(self.register_catalog_inputs()?);
        for (prepared, _) in &requests {
            inputs.push(prepared.request.manifest.clone());
            inputs.push(prepared.request.artifact.clone());
            inputs.extend(prepared.request.companion.iter().cloned());
        }
        if requests
            .iter()
            .any(|(prepared, _)| prepared.requires_reviewed_interfaces())
        {
            let interfaces = self.session.project.interfaces.as_ref().ok_or_else(|| {
                crate::Error::invalid("runtime table replay requires configured [interfaces]")
            })?;
            if interfaces.pack.is_none() {
                return Err(crate::Error::invalid(
                    "runtime table replay requires a reviewed interface pack",
                ));
            }
            inputs.push(interfaces.facts.clone());
            inputs.extend(interfaces.pack.iter().cloned());
            inputs.extend(interfaces.semantic_catalogs.iter().cloned());
        }
        let outputs = requests
            .iter()
            .map(|(_, output)| output.clone())
            .collect::<Vec<_>>();
        if let Some(run) =
            self.plan_stage("event-replays", "event-replays", check, &inputs, &outputs)?
        {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit("event-replays", check, &inputs, &outputs)? {
            return Ok(run);
        }
        for (prepared, output) in requests {
            let document = crate::application::event_replay::execute_prepared(
                prepared,
                &self.session.mmio,
                &self.session.target,
                Some(&self.session.project),
            )?;
            crate::application::event_replay::publish(&document, &output, check)?;
        }
        self.cache_record("event-replays", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn build_review_scopes(&mut self, check: bool) -> Result<StageRun> {
        let mut inputs = self.common_inputs();
        inputs.extend(self.all_linked_ir_outputs());
        if let Some(policy) = self
            .session
            .project
            .verification
            .as_ref()
            .and_then(|verification| verification.policy.as_ref())
        {
            inputs.push(policy.clone());
        }
        if let Some(registers) = self.session.project.registers.as_ref() {
            inputs.push(registers.facts.clone());
        }
        let outputs = vec![
            self.session
                .project
                .review
                .as_ref()
                .expect("configured stage")
                .output
                .clone(),
        ];
        if let Some(run) =
            self.plan_stage("review-scopes", "review-scopes", check, &inputs, &outputs)?
        {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit("review-scopes", check, &inputs, &outputs)? {
            return Ok(run);
        }
        build_review_scopes(&self.session.project, check)?;
        self.cache_record("review-scopes", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn build_navigation(&mut self, check: bool) -> Result<StageRun> {
        let mut inputs = self.common_inputs();
        inputs.extend(self.all_linked_ir_outputs());
        if let Some(symbols) = self.session.project.symbol_inventory.as_ref() {
            inputs.push(symbols.output.clone());
        }
        if let Some(interfaces) = self.session.project.interfaces.as_ref() {
            inputs.push(interfaces.facts.clone());
        }
        let outputs = vec![
            self.session
                .project
                .navigation_index
                .as_ref()
                .expect("configured stage")
                .output
                .clone(),
        ];
        if let Some(run) = self.plan_stage(
            "navigation-index",
            "navigation-index",
            check,
            &inputs,
            &outputs,
        )? {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit("navigation-index", check, &inputs, &outputs)? {
            return Ok(run);
        }
        build_navigation(&self.session.project, check)?;
        self.cache_record("navigation-index", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn validate_code(&mut self, deny_unreviewed: bool) -> Result<StageRun> {
        let mut inputs = self.common_inputs();
        let code = self
            .session
            .project
            .code
            .as_ref()
            .expect("configured stage");
        inputs.push(code.pack.clone());
        inputs.push(
            self.session
                .project
                .symbol_inventory
                .as_ref()
                .expect("dependency checked")
                .output
                .clone(),
        );
        let stage = validation_key("code-boundary-validation", deny_unreviewed);
        if let Some(run) =
            self.plan_stage("code-boundary-validation", &stage, self.check, &inputs, &[])?
        {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit(&stage, self.check, &inputs, &[])? {
            return Ok(run);
        }
        successful(validate_code_boundaries(
            &self.session.project,
            deny_unreviewed,
        )?)?;
        self.cache_record(&stage, self.check, &inputs, &[])?;
        Ok(StageRun::Executed)
    }

    fn review_code(&mut self, check: bool) -> Result<StageRun> {
        let mut inputs = self.common_inputs();
        let code = self
            .session
            .project
            .code
            .as_ref()
            .expect("configured stage");
        inputs.push(code.pack.clone());
        inputs.push(
            self.session
                .project
                .symbol_inventory
                .as_ref()
                .expect("dependency checked")
                .output
                .clone(),
        );
        let outputs = code.review_output.iter().cloned().collect::<Vec<_>>();
        if let Some(run) = self.plan_stage(
            "code-boundary-review",
            "code-boundary-review",
            check,
            &inputs,
            &outputs,
        )? {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit("code-boundary-review", check, &inputs, &outputs)? {
            return Ok(run);
        }
        successful(review_code_boundaries(&self.session.project, check)?)?;
        self.cache_record("code-boundary-review", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn validate_registers(&mut self, deny_unreviewed: bool) -> Result<StageRun> {
        let inputs = self.register_workspace_inputs(false)?;
        let stage = validation_key("register-validation", deny_unreviewed);
        if let Some(run) =
            self.plan_stage("register-validation", &stage, self.check, &inputs, &[])?
        {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit(&stage, self.check, &inputs, &[])? {
            return Ok(run);
        }
        successful(validate_registers(
            &self.session.project,
            self.session.memory_map.as_ref(),
            deny_unreviewed,
        )?)?;
        self.cache_record(&stage, self.check, &inputs, &[])?;
        Ok(StageRun::Executed)
    }

    fn review_registers(&mut self, check: bool) -> Result<StageRun> {
        let inputs = self.register_workspace_inputs(true)?;
        let outputs = self
            .session
            .project
            .registers
            .as_ref()
            .expect("configured stage")
            .review_output
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        if let Some(run) = self.plan_stage(
            "register-review",
            "register-review",
            check,
            &inputs,
            &outputs,
        )? {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit("register-review", check, &inputs, &outputs)? {
            return Ok(run);
        }
        successful(review_registers(&self.session.project, check)?)?;
        self.cache_record("register-review", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn validate_functions(&mut self, deny_unreviewed: bool) -> Result<StageRun> {
        let inputs = self.function_workspace_inputs()?;
        let stage = validation_key("function-validation", deny_unreviewed);
        if let Some(run) =
            self.plan_stage("function-validation", &stage, self.check, &inputs, &[])?
        {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit(&stage, self.check, &inputs, &[])? {
            return Ok(run);
        }
        self.ensure_function_workspace()?;
        let summary = self
            .functions
            .as_ref()
            .expect("function workspace was loaded")
            .summary();
        successful(
            !deny_unreviewed
                || (summary.unreviewed_functions == 0
                    && summary.unreviewed_contexts == 0
                    && summary.unreviewed_fields == 0
                    && summary.unreviewed_type_fields == 0),
        )?;
        self.cache_record(&stage, self.check, &inputs, &[])?;
        Ok(StageRun::Executed)
    }

    fn review_functions(&mut self, check: bool) -> Result<StageRun> {
        let mut inputs = self.function_workspace_inputs()?;
        inputs.extend(self.reviewed_interface_workspace_inputs());
        let outputs = self
            .session
            .project
            .functions
            .as_ref()
            .expect("configured stage")
            .review_output
            .iter()
            .cloned()
            .collect::<Vec<_>>();
        if let Some(run) = self.plan_stage(
            "function-review",
            "function-review",
            check,
            &inputs,
            &outputs,
        )? {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit("function-review", check, &inputs, &outputs)? {
            self.functions = None;
            return Ok(run);
        }
        self.ensure_function_workspace()?;
        let has_interface_pack = self.has_reviewed_interface_workspace();
        if has_interface_pack {
            self.ensure_interface_workspace()?;
        }
        let workspace = self
            .functions
            .as_ref()
            .expect("function workspace was loaded");
        let interface_links = self
            .interfaces
            .as_ref()
            .map(|interfaces| link_reviewed_interfaces(workspace, interfaces.bindings()))
            .transpose()?;
        let contents = render_function_review(workspace, interface_links.as_deref())?;
        super::super::generated_file::write_or_check(
            &outputs[0],
            &contents,
            check,
            "function review",
        )?;
        self.cache_record("function-review", check, &inputs, &outputs)?;
        // Function validation and review share one heavyweight projection, but
        // no later pipeline stage consumes it. Release it before interface
        // validation instead of extending the cold-run memory peak.
        self.functions = None;
        Ok(StageRun::Executed)
    }

    fn validate_interfaces(&mut self, deny_unreviewed: bool) -> Result<StageRun> {
        let mut inputs = self.common_inputs();
        inputs.extend(self.interface_workspace_inputs());
        let stage = validation_key("interface-validation", deny_unreviewed);
        if let Some(run) =
            self.plan_stage("interface-validation", &stage, self.check, &inputs, &[])?
        {
            return Ok(run);
        }
        if let Some(run) = self.cache_hit(&stage, self.check, &inputs, &[])? {
            return Ok(run);
        }
        self.ensure_interface_workspace()?;
        let summary = self
            .interfaces
            .as_ref()
            .expect("interface workspace was loaded")
            .summary();
        successful(
            !deny_unreviewed || (summary.unreviewed_anchors == 0 && summary.unreviewed_slots == 0),
        )?;
        self.cache_record(&stage, self.check, &inputs, &[])?;
        Ok(StageRun::Executed)
    }
}

fn ensure_stage_input(input: &std::path::Path) -> Result<()> {
    let metadata = std::fs::metadata(input).map_err(|error| {
        crate::Error::invalid(format!(
            "analysis input {} is unavailable: {error}",
            input.display()
        ))
    })?;
    if !metadata.is_file() && !metadata.is_dir() {
        return Err(crate::Error::invalid(format!(
            "analysis input {} is neither a file nor a directory",
            input.display()
        )));
    }
    Ok(())
}

fn ensure_check_outputs(outputs: &[std::path::PathBuf]) -> Result<()> {
    for output in outputs {
        if !output.is_file() {
            return Err(crate::Error::invalid(format!(
                "generated output {} is unavailable for check mode",
                output.display()
            )));
        }
    }
    Ok(())
}

fn successful(value: bool) -> Result<StageRun> {
    if value {
        Ok(StageRun::Executed)
    } else {
        Err(crate::Error::invalid(
            "stage reported an unsuccessful result",
        ))
    }
}

fn validation_key(stage: &str, deny_unreviewed: bool) -> String {
    format!("{stage}:deny-unreviewed={deny_unreviewed}")
}

fn linked_ir_stage_cacheable(stage: &str, semantic_domain: Option<&str>) -> bool {
    let is_linked_ir = stage == "linked-ir" || stage.starts_with("linked-ir:");
    !is_linked_ir || semantic_domain.is_some_and(|domain| !domain.trim().is_empty())
}

pub(crate) fn build_symbol_inventory(
    project: &ProjectSpec,
    run_spec: &RunSpec,
    check: bool,
) -> Result<bool> {
    let output = &project
        .symbol_inventory
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[analysis.symbols] is absent"))?
        .output;
    let inputs = run_spec
        .inputs()
        .iter()
        .map(|input| (input.role.to_string(), input.path.clone()))
        .collect::<Vec<_>>();
    let inventory = build_project_linkage_inventory(&inputs)?;
    let document = build_symbol_inventory_document(&inventory, |_| true)?;
    super::super::generated_file::write_or_check_json(
        output,
        &document,
        check,
        "symbol inventory",
        false,
    )?;
    Ok(true)
}

pub(crate) fn discover_project_mmio(
    project: &ProjectSpec,
    run_spec: &RunSpec,
    memory_map: &MemoryMap,
    svd: &MmioMap,
    check: bool,
    jobs: usize,
) -> Result<bool> {
    let output = &project
        .registers
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?
        .facts;
    let artifacts = run_spec
        .inputs()
        .iter()
        .filter_map(|input| match &input.role {
            InputRole::SourceArtifact(source) => Some((source.to_string(), input.path.clone())),
            _ => None,
        })
        .collect::<Vec<_>>();
    let ranges = memory_map
        .mmio_ranges()?
        .into_iter()
        .map(|(name, start, end)| DiscoveryRange { name, start, end })
        .collect::<Vec<_>>();
    let report = discover_mmio(
        &artifacts,
        &ranges,
        "",
        artifact::CodeSymbolSelection::All,
        svd,
        Some(&EffectiveCodeCatalog::load(project)?),
        crate::analysis::MmioDiscoveryOptions { jobs },
    )?;
    let document = mmio_document(&report)?;
    super::super::generated_file::write_or_check_json(
        output,
        &document,
        check,
        "MMIO discovery report",
        false,
    )?;
    Ok(true)
}

pub(crate) fn discover_project_interfaces_operation(
    project: &ProjectSpec,
    run_spec: &RunSpec,
    check: bool,
) -> Result<bool> {
    let output = &project
        .interfaces
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[interfaces] is absent"))?
        .facts;
    let inputs = run_spec
        .inputs()
        .iter()
        .filter(|input| input.role.is_scannable())
        .map(|input| (input.role.to_string(), input.path.clone()))
        .collect::<Vec<_>>();
    if inputs.is_empty() {
        return Err(crate::Error::invalid(
            "run spec has no artifact or inventory inputs for interface discovery",
        ));
    }
    let discovery = discover_project_interfaces(
        &inputs,
        &ProjectInterfaceDiscoveryOptions::default(),
        Some(&EffectiveCodeCatalog::load(project)?),
    )?;
    let document = build_interface_facts(&discovery)?;
    super::super::generated_file::write_or_check_json(
        output,
        &document,
        check,
        "interface discovery report",
        false,
    )?;
    if !discovery.decode_blockers.is_empty() || !discovery.failures.is_empty() {
        tracing::warn!(
            decode_blockers = discovery.decode_blockers.len(),
            analysis_failures = discovery.failures.len(),
            "interface discovery retained partial findings"
        );
    }
    Ok(true)
}

pub(crate) fn build_navigation(project: &ProjectSpec, check: bool) -> Result<bool> {
    let output = &project
        .navigation_index
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[analysis.navigation] is absent"))?
        .output;
    let document = crate::navigation::build(project)?;
    super::super::generated_file::write_or_check_json(
        output,
        &document,
        check,
        "navigation index",
        false,
    )?;
    Ok(true)
}

pub(crate) fn build_review_scopes(project: &ProjectSpec, check: bool) -> Result<bool> {
    let output = &project
        .review
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[review] is absent"))?
        .output;
    let document = crate::review_scopes::build_document(project)?;
    super::super::generated_file::write_or_check_json(
        output,
        &document,
        check,
        "review scope report",
        true,
    )?;
    Ok(true)
}

pub(crate) fn validate_code_boundaries(
    project: &ProjectSpec,
    deny_unreviewed: bool,
) -> Result<bool> {
    let paths = project
        .code
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[code] is absent"))?;
    let inventory = &project
        .symbol_inventory
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[analysis.symbols] is absent"))?
        .output;
    let facts = crate::artifacts::symbol_inventory::load_code_boundary_facts(inventory)?;
    let workspace = CodeWorkspace::load(&facts, &paths.pack, &project.id)?;
    Ok(!deny_unreviewed || workspace.summary().unreviewed == 0)
}

pub(crate) fn review_code_boundaries(project: &ProjectSpec, check: bool) -> Result<bool> {
    let paths = project
        .code
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[code] is absent"))?;
    let output = paths
        .review_output
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[code.review] is absent"))?;
    let inventory = &project
        .symbol_inventory
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[analysis.symbols] is absent"))?
        .output;
    let facts = crate::artifacts::symbol_inventory::load_code_boundary_facts(inventory)?;
    let workspace = CodeWorkspace::load(&facts, &paths.pack, &project.id)?;
    let contents = render_code_boundary_review(&workspace, inventory)?;
    super::super::generated_file::write_or_check(output, &contents, check, "code-boundary review")?;
    Ok(true)
}

pub(crate) fn validate_registers(
    project: &ProjectSpec,
    memory_map: Option<&MemoryMap>,
    deny_unreviewed: bool,
) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?;
    let workspace = ProjectRegisterWorkspace::load(paths)?;
    let summary = workspace.summary()?;
    validate_pac_api(paths)?;
    validate_register_lints(paths)?;
    validate_register_memory_map(paths, memory_map)?;
    validate_register_evidence(paths, memory_map)?;
    Ok(!deny_unreviewed || summary.unreviewed == 0)
}

pub(crate) fn review_registers(project: &ProjectSpec, check: bool) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?;
    let output = paths
        .review_output
        .as_deref()
        .ok_or_else(|| crate::Error::invalid("[registers.review] is absent"))?;
    if !RegisterModel::is_model_file(&paths.model)? {
        return Err(crate::Error::invalid(
            "registers review requires a register-model-v2 manifest",
        ));
    }
    let facts = RegisterFacts::load(&paths.facts)?;
    let model = load_effective_register_model(paths)?;
    let (contents, _) = render_register_review(
        &facts,
        &model,
        &paths.review_ir_reports,
        &paths.owned_ranges,
        &paths.non_operational_functions,
        &paths.facts,
        &paths.model,
    )?;
    super::super::generated_file::write_or_check(output, &contents, check, "register review")?;
    Ok(true)
}

#[cfg(test)]
mod cache_domain_tests {
    use super::{
        ensure_unique_replay_outputs, linked_ir_stage_cacheable, register_catalog_input_paths,
        stage_configuration,
    };
    use crate::{
        function_workspace::{ReviewedEventReplay, ReviewedEventStateModel},
        project::{
            CodeWorkspacePaths, FunctionWorkspacePaths, ProjectSpec, ReviewScopeSpec,
            ReviewWorkspaceSpec,
        },
        project_analysis::NavigationIndexSpec,
        project_ir::{ProjectIrProfile, ProjectIrRoots},
    };

    fn project(id: &str) -> ProjectSpec {
        ProjectSpec {
            id: id.to_owned(),
            target_spec: "target.toml".into(),
            ecosystem_packs: Vec::new(),
            chip_pack: None,
            run_spec: None,
            memory_map: None,
            svd_paths: Vec::new(),
            reviewed_knowledge: Vec::new(),
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
    fn linked_ir_stage_cache_requires_a_stable_semantic_domain() {
        for stage in ["linked-ir", "linked-ir:rom-all"] {
            assert!(!linked_ir_stage_cacheable(stage, None));
            assert!(!linked_ir_stage_cacheable(stage, Some("")));
            assert!(!linked_ir_stage_cacheable(stage, Some("  \t")));
            assert!(linked_ir_stage_cacheable(stage, Some("provider/riscv/v2")));
        }
        assert!(linked_ir_stage_cacheable("symbol-inventory", None));
    }

    #[test]
    fn project_identity_invalidates_every_stage_whose_semantics_embed_it() {
        let mut before = project("vendor-a");
        before.code = Some(CodeWorkspacePaths {
            pack: "code.toml".into(),
            review_output: None,
        });
        let mut after = before.clone();
        after.id = "vendor-b".to_owned();
        for stage in [
            "mmio-discovery",
            "interface-discovery",
            "linked-ir",
            "review-scopes",
            "code-boundary-validation:deny-unreviewed=false",
            "code-boundary-review",
        ] {
            assert_ne!(
                stage_configuration(&before, stage, None),
                stage_configuration(&after, stage, None),
                "{stage} must include project.id in its cache domain"
            );
        }
        assert_eq!(
            stage_configuration(&before, "symbol-inventory", None),
            stage_configuration(&after, "symbol-inventory", None),
            "project.id must not invalidate unrelated artifact discovery"
        );
    }

    #[test]
    fn navigation_cache_tracks_linked_ir_profile_bindings_and_order() {
        let mut before = project("vendor");
        before.navigation_index = Some(NavigationIndexSpec {
            output: "navigation.json".into(),
        });
        before.ir_profiles = vec![
            ProjectIrProfile {
                id: "rom".to_owned(),
                sources: vec!["rom".to_owned()],
                roots: ProjectIrRoots::All,
                include_reachable: true,
                entry_contract: "none".to_owned(),
                output: "rom.ir".into(),
            },
            ProjectIrProfile {
                id: "ram".to_owned(),
                sources: vec!["ram".to_owned()],
                roots: ProjectIrRoots::All,
                include_reachable: true,
                entry_contract: "none".to_owned(),
                output: "ram.ir".into(),
            },
        ];

        let mut renamed = before.clone();
        renamed.ir_profiles[0].id = "boot-rom".to_owned();
        assert_ne!(
            stage_configuration(&before, "navigation-index", None),
            stage_configuration(&renamed, "navigation-index", None),
            "navigation embeds profile IDs"
        );

        let mut reordered = before.clone();
        reordered.ir_profiles.swap(0, 1);
        assert_ne!(
            stage_configuration(&before, "navigation-index", None),
            stage_configuration(&reordered, "navigation-index", None),
            "navigation preserves manifest profile order in its input document"
        );
    }

    #[test]
    fn review_and_function_caches_preserve_profile_to_output_associations() {
        let mut before = project("vendor");
        before.ir_profiles = vec![
            ProjectIrProfile {
                id: "rom".to_owned(),
                sources: vec!["rom".to_owned()],
                roots: ProjectIrRoots::All,
                include_reachable: true,
                entry_contract: "none".to_owned(),
                output: "rom.ir".into(),
            },
            ProjectIrProfile {
                id: "ram".to_owned(),
                sources: vec!["ram".to_owned()],
                roots: ProjectIrRoots::All,
                include_reachable: true,
                entry_contract: "none".to_owned(),
                output: "ram.ir".into(),
            },
        ];
        before.functions = Some(FunctionWorkspacePaths {
            pack: "functions.toml".into(),
            profiles: vec!["rom".to_owned(), "ram".to_owned()],
            review_output: None,
        });
        before.review = Some(ReviewWorkspaceSpec {
            output: "review.json".into(),
            publication_scopes: Vec::new(),
            scopes: vec![ReviewScopeSpec {
                id: "all".to_owned(),
                profiles: vec!["rom".to_owned(), "ram".to_owned()],
                roots: Vec::new(),
                include_reachable: true,
            }],
        });

        let mut swapped = before.clone();
        let first = swapped.ir_profiles[0].output.clone();
        swapped.ir_profiles[0].output = swapped.ir_profiles[1].output.clone();
        swapped.ir_profiles[1].output = first;

        for stage in ["review-scopes", "function-validation", "function-review"] {
            assert_ne!(
                stage_configuration(&before, stage, None),
                stage_configuration(&swapped, stage, None),
                "{stage} must retain each logical profile's report binding"
            );
        }
    }

    #[test]
    fn distinct_replays_cannot_publish_the_same_evidence_path() {
        let replay = |manifest: &str, source: &str| ReviewedEventReplay {
            manifest: manifest.into(),
            source: source.to_owned(),
            evidence: "generated/replay.json".into(),
            producer_phase: "produce".to_owned(),
            consumer_phase: "consume".to_owned(),
            state_observation: "state".to_owned(),
            state_model: ReviewedEventStateModel::CountedLatch,
        };
        let first = replay("first.toml", "rom");
        let duplicate = first.clone();
        let conflicting = replay("second.toml", "ram");

        ensure_unique_replay_outputs([&first, &duplicate]).unwrap();
        let error = ensure_unique_replay_outputs([&first, &conflicting]).unwrap_err();
        assert!(error.to_string().contains("generated/replay.json"));
    }

    #[test]
    fn register_catalog_inputs_include_model_fragments() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-register-catalog-inputs-{}",
            std::process::id()
        ));
        let model = directory.join("device.toml");
        let fragment = directory.join("peripherals/baseband.toml");
        std::fs::create_dir_all(fragment.parent().unwrap()).unwrap();
        std::fs::write(
            &model,
            "schema = 2\nfragments = [\"peripherals/baseband.toml\"]\n\n[device]\nname = \"device\"\nversion = \"1\"\ndescription = \"device\"\naddress-unit-bits = 8\nwidth = 32\n",
        )
        .unwrap();
        let svd = directory.join("public.svd");
        let reviewed = directory.join("reviewed/radio.toml");
        let registers = crate::project::RegisterWorkspacePaths {
            facts: directory.join("mmio.json"),
            model: model.clone(),
            owned_ranges: vec!["radio".to_owned()],
            non_operational_functions: Vec::new(),
            review_output: None,
            review_ir_reports: Vec::new(),
            svd_output: None,
            pac_raw: None,
            bindings: None,
            api_pack: None,
            api_output: None,
            lint_pack: None,
            evidence_catalogs: Vec::new(),
            reviewed_knowledge: vec![reviewed.clone()],
        };

        let inputs =
            register_catalog_input_paths(std::slice::from_ref(&svd), Some(&registers)).unwrap();
        std::fs::remove_dir_all(&directory).unwrap();

        assert_eq!(inputs, vec![svd, model, fragment, reviewed]);
    }
}
