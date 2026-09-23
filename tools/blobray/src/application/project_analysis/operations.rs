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

#[cfg(test)]
use super::ownership::ensure_unique_replay_outputs;
use super::ownership::{OutputCatalog, ReplayDeclarations};
use super::pass_spec::{CachePass, PassKind, stage_configuration};
use super::work::{ExecutionWork, ResolvedWork, WorkDecision};
use super::{ProjectAnalysisInput, ProjectAnalysisInputRequirement};
use super::{
    ProjectAnalysisInputs, ProjectAnalysisOperations, ProjectAnalysisPlanAction,
    ProjectAnalysisPlanReport, ProjectAnalysisPlanWorkItem, ProjectAnalysisPlanner,
    ProjectAnalysisReport, ProjectAnalysisRequest,
    cache::{PipelineInputObservation, ProjectAnalysisCache, ProjectAnalysisCachePlan},
};
use crate::application::{ProjectSession, pipeline::StageRun};
use crate::application::{
    generated_file::GeneratedOutput,
    output_set::{OutputReceipt, OutputSet},
};

pub(crate) fn analyze_project(
    session: &ProjectSession,
    request: ProjectAnalysisRequest,
) -> ProjectAnalysisReport {
    let replays = ReplayDeclarations::capture(&session.project);
    let inputs = project_analysis_inputs(session, &replays);
    let graph = super::pass_spec::PassGraph::resolve(&session.project, inputs);
    let compiled_knowledge_identity =
        crate::providers::analysis_cache_identity(session.target.knowledge_provider.as_deref());
    let (pipeline_inputs, pipeline_input_error, output_catalog) =
        pipeline_input_observation(session, &replays);
    let mut operations = ResolvedProjectAnalysisOperations {
        session,
        cache: if request.check {
            ProjectAnalysisCache::disabled()
        } else {
            ProjectAnalysisCache::deferred(&session.manifest)
        }
        .with_active_applicability_fingerprint(session.active_applicability_identity())
        .with_compiled_knowledge_identity(compiled_knowledge_identity),
        check: request.check,
        functions: None,
        interfaces: None,
        planner: None,
        pipeline_inputs,
        pipeline_input_error,
        output_catalog,
        replays,
        coverage_report: None,
        completed_outputs: Vec::new(),
        captures: std::sync::OnceLock::new(),
    };
    let mut report = super::run(&graph, request, &mut operations);
    report.coverage = operations.coverage_report.take().unwrap_or_else(|| {
        crate::application::coverage::inspect(&session.project, session.run_spec.as_ref())
    });
    report.next_steps = super::follow_up_steps(&report, &session.context());
    report
}

pub(crate) fn plan_project(
    session: &ProjectSession,
    request: ProjectAnalysisRequest,
) -> ProjectAnalysisPlanReport {
    let replays = ReplayDeclarations::capture(&session.project);
    let inputs = project_analysis_inputs(session, &replays);
    let graph = super::pass_spec::PassGraph::resolve(&session.project, inputs);
    let compiled_knowledge_identity =
        crate::providers::analysis_cache_identity(session.target.knowledge_provider.as_deref());
    let (pipeline_inputs, pipeline_input_error, output_catalog) =
        pipeline_input_observation(session, &replays);
    let mut operations = ResolvedProjectAnalysisOperations {
        session,
        cache: ProjectAnalysisCache::planning(&session.manifest)
            .with_active_applicability_fingerprint(session.active_applicability_identity())
            .with_compiled_knowledge_identity(compiled_knowledge_identity),
        check: request.check,
        functions: None,
        interfaces: None,
        planner: Some(ProjectAnalysisPlanner::default()),
        pipeline_inputs,
        pipeline_input_error,
        output_catalog,
        replays,
        coverage_report: None,
        completed_outputs: Vec::new(),
        captures: std::sync::OnceLock::new(),
    };
    let mut execution = super::run(&graph, request, &mut operations);
    execution.coverage =
        crate::application::coverage::declarations(&session.project, session.run_spec.as_ref());
    operations
        .planner
        .take()
        .expect("project analysis planner was configured")
        .finish(&graph, execution)
}

fn project_analysis_inputs(
    session: &ProjectSession,
    replays: &ReplayDeclarations,
) -> ProjectAnalysisInputs {
    let replay_requirements = match replays.records() {
        Ok(replays) => (
            !replays.is_empty(),
            replays.iter().any(|replay| {
                crate::application::event_replay::manifest_requires_reviewed_interfaces(
                    &replay.manifest,
                )
                .unwrap_or(true)
            }),
        ),
        Err(_) => (true, true),
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
    replays: &ReplayDeclarations,
) -> (
    Option<PipelineInputObservation>,
    Option<String>,
    Option<OutputCatalog>,
) {
    let result = (|| -> Result<_> {
        let paths = pipeline_input_paths(session, replays)?;
        let observation = PipelineInputObservation::capture(paths.clone())?;
        let outputs = OutputCatalog::capture(&session.project, &paths, replays)?;
        session.validate_active_artifacts()?;
        Ok((observation, outputs))
    })();
    match result {
        Ok((observation, outputs)) => (Some(observation), None, Some(outputs)),
        Err(error) => (None, Some(error.to_string()), None),
    }
}

fn pipeline_input_paths(
    session: &ProjectSession,
    replays: &ReplayDeclarations,
) -> Result<Vec<std::path::PathBuf>> {
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
    if session
        .project
        .functions
        .as_ref()
        .is_some_and(|functions| functions.pack.is_file())
    {
        paths.extend(
            replays
                .records()?
                .iter()
                .map(|replay| replay.manifest.clone()),
        );
    }
    Ok(paths)
}

struct ResolvedProjectAnalysisOperations<'a> {
    captures: std::sync::OnceLock<crate::source_set::CapturedSourceSet>,
    session: &'a ProjectSession,
    cache: ProjectAnalysisCache,
    check: bool,
    functions: Option<FunctionWorkspace>,
    interfaces: Option<InterfaceWorkspace>,
    planner: Option<ProjectAnalysisPlanner>,
    pipeline_inputs: Option<PipelineInputObservation>,
    pipeline_input_error: Option<String>,
    output_catalog: Option<OutputCatalog>,
    replays: ReplayDeclarations,
    completed_outputs: Vec<OutputReceipt>,
    coverage_report: Option<Vec<crate::application::CoverageObligation>>,
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

fn append_interface_workspace_inputs(
    paths: &mut Vec<std::path::PathBuf>,
    interfaces: &crate::project::InterfaceWorkspacePaths,
) {
    paths.push(interfaces.facts.clone());
    paths.extend(interfaces.pack.iter().cloned());
    paths.extend(interfaces.semantic_catalogs.iter().cloned());
    paths.extend(interfaces.capability_packs.iter().cloned());
    paths.extend(interfaces.interface_template_packs.iter().cloned());
}

fn append_register_workspace_inputs(
    paths: &mut Vec<std::path::PathBuf>,
    registers: &crate::project::RegisterWorkspacePaths,
) -> Result<()> {
    paths.push(registers.facts.clone());
    paths.extend(RegisterModel::input_paths(&registers.model)?);
    paths.extend(registers.reviewed_knowledge.iter().cloned());
    paths.extend(registers.ownership_policy.iter().cloned());
    paths.extend(registers.api_pack.iter().cloned());
    paths.extend(registers.lint_pack.iter().cloned());
    paths.extend(registers.evidence_catalogs.iter().cloned());
    Ok(())
}

impl ResolvedProjectAnalysisOperations<'_> {
    fn captured_sources(&self) -> &crate::source_set::CapturedSourceSet {
        self.captures.get_or_init(|| {
            crate::source_set::CapturedSourceSet::capture(
                self.session
                    .run_spec
                    .iter()
                    .flat_map(|run| run.inputs())
                    .map(|input| input.path.clone()),
            )
        })
    }

    fn resolve_work(
        &self,
        key: &str,
        check: bool,
        inputs: Vec<std::path::PathBuf>,
        outputs: Vec<std::path::PathBuf>,
    ) -> Result<ResolvedWork> {
        let owner = CachePass::parse(key)?.spec.kind;
        let inputs = inputs
            .into_iter()
            .map(|path| {
                let optional = owner == PassKind::LinkedIr
                    && self.session.project.code.is_none()
                    && self
                        .session
                        .project
                        .symbol_inventory
                        .as_ref()
                        .is_some_and(|symbols| symbols.output == path);
                ProjectAnalysisInput {
                    path,
                    requirement: if optional {
                        ProjectAnalysisInputRequirement::Optional
                    } else {
                        ProjectAnalysisInputRequirement::Required
                    },
                }
            })
            .collect();
        ResolvedWork::new(
            key.to_owned(),
            self.stage_configuration(key)?,
            inputs,
            outputs,
            check,
            self.linked_ir_semantic_cache_domain(),
        )
    }

    fn prepare_work(&mut self, work: ResolvedWork) -> Result<WorkDecision> {
        self.output_catalog
            .as_ref()
            .ok_or_else(|| crate::Error::invalid("output ownership preflight failed"))?
            .validate(&work)?;
        if let Some(run) = self.plan_work(&work)? {
            return Ok(WorkDecision::Complete(run));
        }
        if let Some(run) = self.cached_work(&work)? {
            return Ok(WorkDecision::Complete(run));
        }
        Ok(WorkDecision::Execute(work.prepared()?))
    }

    fn plan_work(&mut self, work: &ResolvedWork) -> Result<Option<StageRun>> {
        let stage = work.owner().spec().name;
        let cache_stage = work.key();
        let inputs = work.input_paths();
        let outputs = work.outputs();
        let check = work.check();
        let Some(planner) = self.planner.as_ref() else {
            return Ok(None);
        };
        self.cache.ensure_planning_snapshot()?;
        let catalog = self
            .output_catalog
            .as_ref()
            .expect("output ownership was validated");
        let materializations = inputs
            .iter()
            .map(|input| {
                Ok(catalog
                    .producer(input)?
                    .filter(|owner| planner.materializes(&owner.work))
                    .map(|owner| (input.clone(), owner.clone())))
            })
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .flatten()
            .collect::<Vec<_>>();
        // A producer can defer only the paths that this work declares.
        let deferred = materializations
            .iter()
            .map(|(path, _)| path.clone())
            .collect::<Vec<_>>();
        work.validate_inputs(&deferred)?;
        if materializations.is_empty() && check {
            ensure_check_outputs(outputs)?;
        }
        let awaiting_inputs = materializations
            .iter()
            .map(|(input, owner)| super::ProjectAnalysisPlanAwaitingInput {
                path: input.clone(),
                producer_stage: owner.pass.spec().name.to_owned(),
                producer_work: owner.work.clone(),
            })
            .collect::<Vec<_>>();
        let (action, signature, cause) = if !materializations.is_empty() {
            let producers = materializations
                .iter()
                .map(|(_, owner)| owner.pass.spec().name)
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
        } else if !work.cacheable() {
            (
                ProjectAnalysisPlanAction::Compute,
                None,
                Some(
                    "persistent cache is disabled because the selected harness has no stable semantic cache domain"
                        .to_owned(),
                ),
            )
        } else {
            match self
                .cache
                .plan(cache_stage, work.configuration(), inputs, outputs)?
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
                    inputs: work.inputs().to_vec(),
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
        paths.extend(project.reviewed_knowledge.iter().cloned());
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
            .filter(|_| super::pass_spec::linked_ir_uses_reviewed_interfaces(project))
        {
            append_interface_workspace_inputs(&mut paths, interfaces);
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
            append_interface_workspace_inputs(&mut paths, interfaces);
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
            self.interfaces = Some(InterfaceWorkspace::load_with_templates(
                &paths.facts,
                pack,
                &paths.semantic_catalogs,
                &paths.interface_template_packs,
                self.session.target.calling_convention.label(),
                self.session
                    .target
                    .knowledge_provider
                    .as_deref()
                    .map(crate::providers::contracts)
                    .transpose()?,
            )?);
        }
        Ok(())
    }

    fn register_workspace_inputs(&self, include_ir: bool) -> Result<Vec<std::path::PathBuf>> {
        let mut paths = self.common_inputs();
        paths.extend(self.session.project.memory_map.iter().cloned());
        if let Some(registers) = self.session.project.registers.as_ref() {
            append_register_workspace_inputs(&mut paths, registers)?;
            if include_ir {
                paths.extend(registers.review_ir_reports.iter().cloned());
            }
        }
        Ok(paths)
    }

    fn cached_work(&mut self, work: &ResolvedWork) -> Result<Option<StageRun>> {
        let stage = work.key();
        let inputs = work.input_paths();
        let outputs = work.outputs();
        let check = work.check();
        work.validate_inputs(&[])?;
        if check {
            return Ok(None);
        }
        if !work.cacheable() {
            tracing::warn!(
                cache_stage = stage,
                cache_outcome = "bypass",
                "persistent linked-IR stage cache disabled: the selected RISC-V harness has no stable semantic cache domain"
            );
            return Ok(None);
        }
        let destinations = OutputSet::new(outputs, false)?;
        let current = self
            .cache
            .is_current(stage, work.configuration(), inputs, &destinations)?;
        if current {
            self.completed_outputs.extend(destinations.receipts()?);
        }
        if current {
            let run = if self.cache.last_lookup_restored() {
                tracing::info!(
                    cache_stage = stage,
                    cache_outcome = "hit-restored",
                    "persistent stage cache hit; restored content-addressed outputs"
                );
                StageRun::Restored
            } else {
                tracing::info!(
                    cache_stage = stage,
                    cache_outcome = "hit-current",
                    "persistent stage cache hit; outputs are current"
                );
                StageRun::Current
            };
            Ok(Some(run))
        } else {
            tracing::info!(
                cache_stage = stage,
                cache_outcome = "miss-recompute",
                "persistent stage cache miss; recomputing stage"
            );
            Ok(None)
        }
    }

    fn complete_work(&mut self, execution: ExecutionWork) -> Result<()> {
        let (work, receipts) = execution.finish()?;
        if !work.check() && work.cacheable() {
            self.cache.record(
                work.key(),
                work.configuration(),
                work.input_paths(),
                &receipts,
            )?;
            tracing::info!(
                cache_stage = work.key(),
                cache_outcome = "recomputed-published",
                "published recomputed stage to the persistent cache"
            );
        }
        self.completed_outputs.extend(receipts);
        Ok(())
    }

    fn linked_ir_semantic_cache_domain(&self) -> Option<&'static str> {
        crate::providers::riscv_or_neutral(self.session.target.knowledge_provider.as_deref())
            .ok()
            .map(|harness| harness.semantic_cache_domain)
    }

    /// Stable, stage-owned project configuration included in the cache key.
    ///
    /// File contents remain explicit inputs. Keeping unrelated manifest
    /// sections out of this value prevents, for example, a review-scope edit
    /// from invalidating artifact-wide decoding and linked IR.
    fn stage_configuration(&self, stage: &str) -> Result<String> {
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
        crate::artifacts::bundle_files(&profile.output)
            .chain(std::iter::once(
                profile.output.join(crate::application::coverage::FILE),
            ))
            .collect()
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

impl ProjectAnalysisOperations for ResolvedProjectAnalysisOperations<'_> {
    fn complete_analysis_epoch(&mut self) -> Result<()> {
        // Plan uses the shared pass traversal, but it owns no publication
        // capability and must not open a writer at the simulated success edge.
        if self.planner.is_some() {
            return Ok(());
        }
        for receipt in &self.completed_outputs {
            receipt.validate()?;
        }
        self.cache.publish_analysis_outputs(&self.completed_outputs)
    }

    fn validate_pipeline_inputs(&mut self) -> Result<()> {
        if let Some(error) = self.pipeline_input_error.as_deref() {
            return Err(crate::Error::invalid(format!(
                "project analysis preflight failed: {error}"
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
        let work = self.resolve_work("symbol-inventory", check, inputs, outputs)?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        build_symbol_inventory(
            self.captured_sources(),
            self.run_spec()?,
            execution.outputs().file(0, "symbol inventory")?,
        )?;
        self.complete_work(execution)?;
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
        let work = self.resolve_work("mmio-discovery", check, inputs, outputs)?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        discover_project_mmio(
            self.captured_sources(),
            &self.session.project,
            self.run_spec()?,
            self.memory_map()?,
            &self.session.mmio,
            execution.outputs().file(0, "MMIO discovery report")?,
            jobs,
        )?;
        self.complete_work(execution)?;
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
        let work = self.resolve_work("interface-discovery", check, inputs, outputs)?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        discover_project_interfaces_operation(
            self.captured_sources(),
            &self.session.project,
            self.run_spec()?,
            execution.outputs().file(0, "interface discovery report")?,
        )?;
        self.complete_work(execution)?;
        Ok(StageRun::Executed)
    }

    fn coverage(&mut self) -> Result<StageRun> {
        let project = &self.session.project;
        let run = self.session.run_spec.as_ref();
        crate::application::coverage::require_complete(
            &crate::application::coverage::declarations(project, run),
        )?;
        let profile_inputs = if self.planner.is_some() {
            project
                .ir_profiles
                .iter()
                .map(|profile| {
                    let mut inputs = self
                        .session
                        .run_spec_path
                        .iter()
                        .cloned()
                        .collect::<Vec<_>>();
                    inputs.extend(crate::application::project_ir_build::profile_input_paths(
                        profile,
                        self.run_spec()?,
                    )?);
                    inputs.extend(self.linked_ir_outputs(profile));
                    inputs.sort();
                    inputs.dedup();
                    Ok(inputs
                        .into_iter()
                        .map(|path| ProjectAnalysisInput {
                            path,
                            requirement: ProjectAnalysisInputRequirement::Required,
                        })
                        .collect::<Vec<_>>())
                })
                .collect::<Result<Vec<_>>>()?
        } else {
            Vec::new()
        };
        if let Some(planner) = self.planner.as_mut() {
            for family in project.analysis_symbol_families.iter().filter(|family| {
                family.disposition == crate::project::AnalysisSymbolFamilyDisposition::Required
            }) {
                planner.record("analysis-coverage", ProjectAnalysisPlanWorkItem {
                    name: format!("required:{}", family.id), action: ProjectAnalysisPlanAction::Verify,
                    inputs: self.session.run_spec_path.iter().cloned().map(|path| ProjectAnalysisInput { path, requirement: ProjectAnalysisInputRequirement::Required }).collect(),
                    signature: None, outputs: Vec::new(), cause: Some(format!("source-artifact:{}; profile={:?}; every root matching {:?} must have an outcome", family.source, family.profile, family.symbol_prefix)), awaiting_inputs: Vec::new(),
                });
            }
            for (profile, inputs) in project.ir_profiles.iter().zip(profile_inputs) {
                crate::application::project_inputs::resolve_inputs(
                    profile,
                    run.ok_or_else(|| crate::Error::invalid("run-spec is not configured"))?,
                )?;
                let catalog = self.output_catalog.as_ref().expect("preflight succeeded");
                let awaiting_inputs = inputs
                    .iter()
                    .map(|input| {
                        Ok(catalog
                            .producer(&input.path)?
                            .filter(|owner| planner.materializes(&owner.work))
                            .map(|owner| super::ProjectAnalysisPlanAwaitingInput {
                                path: input.path.clone(),
                                producer_stage: owner.pass.spec().name.to_owned(),
                                producer_work: owner.work.clone(),
                            }))
                    })
                    .collect::<Result<Vec<_>>>()?
                    .into_iter()
                    .flatten()
                    .collect();
                planner.record("analysis-coverage", ProjectAnalysisPlanWorkItem {
                    name: format!("coverage:{}", profile.id), action: ProjectAnalysisPlanAction::Verify,
                    signature: None, inputs, outputs: Vec::new(),
                    cause: Some("verify every selected root and required family after linked-IR materialization".to_owned()), awaiting_inputs,
                });
            }
            return Ok(StageRun::Executed);
        }
        let coverage = crate::application::coverage::inspect(project, run);
        let complete = crate::application::coverage::require_complete(&coverage);
        self.coverage_report = Some(coverage);
        complete?;
        Ok(if self.check {
            StageRun::Executed
        } else {
            StageRun::Current
        })
    }

    fn build_linked_ir(&mut self, check: bool, jobs: usize) -> Result<StageRun> {
        let profiles = self.session.project.ir_profiles.clone();
        let mut pending = Vec::new();
        let mut restored = false;
        let mut planned = false;
        for profile in profiles {
            let key = format!("linked-ir:{}", profile.id);
            let work = self.resolve_work(
                &key,
                check,
                self.linked_ir_inputs(&profile)?,
                self.linked_ir_outputs(&profile),
            )?;
            match self.prepare_work(work)? {
                WorkDecision::Complete(StageRun::Current) => (),
                WorkDecision::Complete(StageRun::Restored) => restored = true,
                WorkDecision::Complete(StageRun::Executed) => planned = true,
                WorkDecision::Execute(execution) => pending.push(execution),
            }
        }
        if pending.is_empty() {
            return Ok(if planned {
                StageRun::Executed
            } else if restored {
                StageRun::Restored
            } else {
                StageRun::Current
            });
        }
        let request = crate::application::project_ir_build::ProjectIrBuildRequest {
            profiles: pending
                .iter()
                .map(|execution| execution.work().profile().expect("profile work").to_owned())
                .collect(),
            check,
            jobs,
            refresh_review_scopes: false,
        };
        let output_sets = pending
            .iter()
            .map(|execution| {
                (
                    execution.work().profile().expect("profile work").to_owned(),
                    execution.outputs().clone(),
                )
            })
            .collect();
        self.captured_sources();
        let captures = self
            .captures
            .get()
            .expect("source set captured before borrowing writer");
        let session = self.session;
        let function_fact_store = if check {
            None
        } else {
            Some(self.cache.query_store_mut()?)
        };
        crate::application::project_ir_build::build_project_ir_with_outputs(
            crate::application::project_ir_build::ProjectIrBuildContext {
                captures,
                project: &session.project,
                run_spec: session
                    .run_spec
                    .as_ref()
                    .ok_or_else(|| crate::Error::invalid("run-spec is not configured"))?,
                svd: &session.mmio,
                target: &session.target,
            },
            request,
            function_fact_store,
            &output_sets,
        )?;
        for execution in pending {
            self.complete_work(execution)?;
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
        let configured = self.replays.records()?;
        let run_spec = self.run_spec()?;
        if configured.is_empty() {
            return Err(crate::Error::invalid(
                "event-replays stage has no configured reviewed replay",
            ));
        }
        let mut requests = Vec::with_capacity(configured.len());
        for replay in configured.iter().cloned() {
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
            append_interface_workspace_inputs(&mut inputs, interfaces);
        }
        let outputs = requests
            .iter()
            .map(|(_, output)| output.clone())
            .collect::<Vec<_>>();
        let work = self.resolve_work("event-replays", check, inputs, outputs)?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        for (index, (prepared, _)) in requests.into_iter().enumerate() {
            let document = crate::application::event_replay::execute_prepared(
                prepared,
                &self.session.mmio,
                &self.session.target,
                Some(&self.session.project),
            )?;
            crate::application::event_replay::publish(
                &document,
                execution
                    .outputs()
                    .file(index, "execution replay evidence")?,
            )?;
        }
        self.complete_work(execution)?;
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
        let work = self.resolve_work("review-scopes", check, inputs, outputs)?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        build_review_scopes(
            &self.session.project,
            execution.outputs().file(0, "review scope report")?,
        )?;
        self.complete_work(execution)?;
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
        let work = self.resolve_work("navigation-index", check, inputs, outputs)?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        build_navigation(
            &self.session.project,
            execution.outputs().file(0, "navigation index")?,
        )?;
        self.complete_work(execution)?;
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
        let work = self.resolve_work(&stage, self.check, inputs, Vec::new())?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        successful(validate_code_boundaries(
            &self.session.project,
            deny_unreviewed,
        )?)?;
        self.complete_work(execution)?;
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
        let work = self.resolve_work("code-boundary-review", check, inputs, outputs)?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        successful(review_code_boundaries(
            &self.session.project,
            execution.outputs().file(0, "code-boundary review")?,
        )?)?;
        self.complete_work(execution)?;
        Ok(StageRun::Executed)
    }

    fn validate_registers(&mut self, deny_unreviewed: bool) -> Result<StageRun> {
        let inputs = self.register_workspace_inputs(false)?;
        let stage = validation_key("register-validation", deny_unreviewed);
        let work = self.resolve_work(&stage, self.check, inputs, Vec::new())?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        successful(validate_registers(
            &self.session.project,
            self.session.memory_map.as_ref(),
            deny_unreviewed,
        )?)?;
        self.complete_work(execution)?;
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
        let work = self.resolve_work("register-review", check, inputs, outputs)?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        successful(review_registers(
            &self.session.project,
            execution.outputs().file(0, "register review")?,
        )?)?;
        self.complete_work(execution)?;
        Ok(StageRun::Executed)
    }

    fn validate_functions(&mut self, deny_unreviewed: bool) -> Result<StageRun> {
        let inputs = self.function_workspace_inputs()?;
        let stage = validation_key("function-validation", deny_unreviewed);
        let work = self.resolve_work(&stage, self.check, inputs, Vec::new())?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
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
        self.complete_work(execution)?;
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
        let work = self.resolve_work("function-review", check, inputs, outputs)?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                self.functions = None;
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
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
        execution
            .outputs()
            .file(0, "function review")?
            .text(&contents)?;
        self.complete_work(execution)?;
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
        let work = self.resolve_work(&stage, self.check, inputs, Vec::new())?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        self.ensure_interface_workspace()?;
        let summary = self
            .interfaces
            .as_ref()
            .expect("interface workspace was loaded")
            .summary();
        successful(
            !deny_unreviewed || (summary.unreviewed_anchors == 0 && summary.unreviewed_slots == 0),
        )?;
        self.complete_work(execution)?;
        Ok(StageRun::Executed)
    }

    fn build_capability_context(&mut self, check: bool) -> Result<StageRun> {
        let inputs = self.interface_workspace_inputs();
        let output = self
            .session
            .project
            .interfaces
            .as_ref()
            .and_then(|paths| paths.capability_context.as_ref())
            .ok_or_else(|| crate::Error::invalid("[interfaces.capability-context] is absent"))?
            .clone();
        let outputs = vec![output.clone()];
        let work = self.resolve_work("interface-capability-context", check, inputs, outputs)?;
        let execution = match self.prepare_work(work)? {
            WorkDecision::Complete(run) => {
                return Ok(run);
            }
            WorkDecision::Execute(execution) => execution,
        };
        self.ensure_interface_workspace()?;
        crate::application::capability_context::build_and_publish(
            self.session,
            self.interfaces
                .as_ref()
                .expect("interface workspace was loaded"),
            execution
                .outputs()
                .file(0, "interface capability context")?,
        )?;
        self.complete_work(execution)?;
        Ok(StageRun::Executed)
    }
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

pub(crate) fn build_symbol_inventory(
    captures: &crate::source_set::CapturedSourceSet,
    run_spec: &RunSpec,
    output: GeneratedOutput<'_>,
) -> Result<bool> {
    let inputs = run_spec
        .inputs()
        .iter()
        .map(|input| (input.role.to_string(), input.path.clone()))
        .collect::<Vec<_>>();
    let inventory = build_project_linkage_inventory(captures, &inputs)?;
    let document = build_symbol_inventory_document(&inventory, |_| true);
    output.json(&document, false)?;
    Ok(true)
}

pub(crate) fn discover_project_mmio(
    captures: &crate::source_set::CapturedSourceSet,
    project: &ProjectSpec,
    run_spec: &RunSpec,
    memory_map: &MemoryMap,
    svd: &MmioMap,
    output: GeneratedOutput<'_>,
    jobs: usize,
) -> Result<bool> {
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
    let report = discover_mmio(crate::analysis::MmioDiscoveryRequest {
        captures,
        artifacts: &artifacts,
        ranges: &ranges,
        symbol_prefix: "",
        code_symbol_selection: artifact::CodeSymbolSelection::All,
        svd,
        effective_code: Some(&EffectiveCodeCatalog::load(project)?),
        options: crate::analysis::MmioDiscoveryOptions { jobs },
    })?;
    let document = mmio_document(&report)?;
    output.json(&document, false)?;
    Ok(true)
}

pub(crate) fn discover_project_interfaces_operation(
    captures: &crate::source_set::CapturedSourceSet,
    project: &ProjectSpec,
    run_spec: &RunSpec,
    output: GeneratedOutput<'_>,
) -> Result<bool> {
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
        captures,
        &inputs,
        &ProjectInterfaceDiscoveryOptions::default(),
        Some(&EffectiveCodeCatalog::load(project)?),
    )?;
    let document = build_interface_facts(&discovery)?;
    output.json(&document, false)?;
    if !discovery.decode_blockers.is_empty() || !discovery.failures.is_empty() {
        tracing::warn!(
            decode_blockers = discovery.decode_blockers.len(),
            analysis_failures = discovery.failures.len(),
            "interface discovery retained partial findings"
        );
    }
    Ok(true)
}

pub(crate) fn build_navigation(project: &ProjectSpec, output: GeneratedOutput<'_>) -> Result<bool> {
    let document = crate::navigation::build(project)?;
    output.json(&document, false)?;
    Ok(true)
}

pub(crate) fn build_review_scopes(
    project: &ProjectSpec,
    output: GeneratedOutput<'_>,
) -> Result<bool> {
    let document = crate::review_scopes::build_document(project)?;
    output.json(&document, true)?;
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

pub(crate) fn review_code_boundaries(
    project: &ProjectSpec,
    output: GeneratedOutput<'_>,
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
    let contents = render_code_boundary_review(&workspace, inventory)?;
    output.text(&contents)?;
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

pub(crate) fn review_registers(project: &ProjectSpec, output: GeneratedOutput<'_>) -> Result<bool> {
    let paths = project
        .registers
        .as_ref()
        .ok_or_else(|| crate::Error::invalid("[registers] is absent"))?;

    if !RegisterModel::is_model_file(&paths.model)? {
        return Err(crate::Error::invalid(
            "registers review requires a register-model-v3 manifest",
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
    output.text(&contents)?;
    Ok(true)
}

#[cfg(test)]
mod cache_domain_tests {
    use super::{
        append_interface_workspace_inputs, append_register_workspace_inputs,
        ensure_unique_replay_outputs, register_catalog_input_paths, stage_configuration,
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
            manifest: std::path::PathBuf::from("project.toml"),
            loaded_model_inputs: Default::default(),
            id: id.to_owned(),
            target_spec: "target.toml".into(),
            ecosystem_packs: Vec::new(),
            chip_pack: None,
            analysis_provider: None,
            run_spec: None,
            memory_map: None,
            svd_paths: Vec::new(),
            reviewed_knowledge: Vec::new(),
            reviewed_knowledge_default: None,
            review_context: open_radio_vendor_contracts::ApplicabilityContext::default(),
            symbol_inventory: None,
            navigation_index: None,
            code: None,
            ir_profiles: Vec::new(),
            analysis_symbol_families: Vec::new(),
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
            assert!(!super::CachePass::parse(stage).unwrap().cacheable(None));
            assert!(!super::CachePass::parse(stage).unwrap().cacheable(Some("")));
            assert!(
                !super::CachePass::parse(stage)
                    .unwrap()
                    .cacheable(Some("  \t"))
            );
            assert!(
                super::CachePass::parse(stage)
                    .unwrap()
                    .cacheable(Some("provider/riscv/v2"))
            );
        }
        assert!(
            super::CachePass::parse("symbol-inventory")
                .unwrap()
                .cacheable(None)
        );
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
                stage_configuration(&before, stage, None).unwrap(),
                stage_configuration(&after, stage, None).unwrap(),
                "{stage} must include project.id in its cache domain"
            );
        }
        assert_eq!(
            stage_configuration(&before, "symbol-inventory", None).unwrap(),
            stage_configuration(&after, "symbol-inventory", None).unwrap(),
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
            stage_configuration(&before, "navigation-index", None).unwrap(),
            stage_configuration(&renamed, "navigation-index", None).unwrap(),
            "navigation embeds profile IDs"
        );

        let mut reordered = before.clone();
        reordered.ir_profiles.swap(0, 1);
        assert_ne!(
            stage_configuration(&before, "navigation-index", None).unwrap(),
            stage_configuration(&reordered, "navigation-index", None).unwrap(),
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
                protocols: vec!["shared".to_owned()],
                profiles: vec!["rom".to_owned(), "ram".to_owned()],
                roots: Vec::new(),
                include_reachable: true,
            }],
        });

        let mut swapped = before.clone();
        let first = swapped.ir_profiles[0].output.clone();
        swapped.ir_profiles[0].output = swapped.ir_profiles[1].output.clone();
        swapped.ir_profiles[1].output = first;

        for stage in [
            "review-scopes",
            "function-validation:deny-unreviewed=false",
            "function-review",
        ] {
            assert_ne!(
                stage_configuration(&before, stage, None).unwrap(),
                stage_configuration(&swapped, stage, None).unwrap(),
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
            "schema = 3\nchip = \"fixture-chip\"\nfragments = [\"peripherals/baseband.toml\"]\n\n[device]\nname = \"device\"\nversion = \"1\"\ndescription = \"device\"\naddress-unit-bits = 8\nwidth = 32\n",
        )
        .unwrap();
        let svd = directory.join("public.svd");
        let reviewed = directory.join("reviewed/radio.toml");
        let policy = directory.join("ownership.toml");
        std::fs::write(&policy, "schema = 1\nowned-ranges = [\"radio\"]\n").unwrap();
        let registers = crate::project::RegisterWorkspacePaths {
            facts: directory.join("mmio.json"),
            model: model.clone(),
            ownership_policy: Some(policy.clone()),
            owned_ranges: vec!["radio".to_owned()],
            non_operational_functions: Vec::new(),
            review_output: None,
            review_ir_reports: Vec::new(),
            observations: Vec::new(),
            svd_output: None,
            pac_raw: None,
            bindings: None,
            api_pack: None,
            api_output: None,
            lint_pack: None,
            evidence_catalogs: Vec::new(),
            reviewed_knowledge: vec![reviewed.clone()],
            review_context: open_radio_vendor_contracts::ApplicabilityContext::default(),
        };

        let inputs =
            register_catalog_input_paths(std::slice::from_ref(&svd), Some(&registers)).unwrap();
        assert_eq!(inputs, vec![svd, model, fragment, reviewed]);

        let mut inputs = Vec::new();
        append_register_workspace_inputs(&mut inputs, &registers).unwrap();
        assert!(inputs.contains(&policy));
        // Every selected input is pinned by the pipeline observation. Here the
        // policy alone is sufficient to demonstrate mid-run scope mutation.
        let mut observation =
            super::PipelineInputObservation::capture(vec![policy.clone()]).unwrap();
        std::fs::write(&policy, "schema = 1\nowned-ranges = [\"other\"]\n").unwrap();
        assert!(
            observation
                .validate()
                .unwrap_err()
                .to_string()
                .contains("project inputs changed")
        );
        drop(observation);
        std::fs::remove_dir_all(&directory).unwrap();
    }

    #[test]
    fn reusable_interface_packs_are_exact_guarded_stage_inputs() {
        let directory = std::env::temp_dir().join(format!(
            "blobray-capability-stage-input-{}",
            std::process::id()
        ));
        if directory.exists() {
            std::fs::remove_dir_all(&directory).unwrap();
        }
        std::fs::create_dir_all(&directory).unwrap();
        let facts = directory.join("interfaces.json");
        let pack = directory.join("interfaces.toml");
        let semantics = directory.join("semantics.toml");
        let capabilities = directory.join("capabilities.toml");
        let templates = directory.join("interface-templates.toml");
        for path in [&facts, &pack, &semantics, &capabilities, &templates] {
            std::fs::write(path, "generation-a").unwrap();
        }
        let interfaces = crate::project::InterfaceWorkspacePaths {
            facts: facts.clone(),
            pack: Some(pack.clone()),
            capability_context: Some(directory.join("capability-context.json")),
            semantic_catalogs: vec![semantics.clone()],
            capability_packs: vec![capabilities.clone()],
            interface_template_packs: vec![templates.clone()],
        };
        let mut inputs = Vec::new();
        append_interface_workspace_inputs(&mut inputs, &interfaces);
        assert_eq!(
            inputs,
            [
                facts,
                pack,
                semantics,
                capabilities.clone(),
                templates.clone()
            ]
        );

        let mut observation = super::PipelineInputObservation::capture(inputs.clone()).unwrap();
        std::fs::write(&capabilities, "generation-b").unwrap();
        let error = observation.validate().unwrap_err();
        assert!(error.to_string().contains("project inputs changed"));
        drop(observation);

        std::fs::write(&capabilities, "generation-a").unwrap();
        let mut observation = super::PipelineInputObservation::capture(inputs).unwrap();
        std::fs::write(&templates, "generation-b").unwrap();
        let error = observation.validate().unwrap_err();
        assert!(error.to_string().contains("project inputs changed"));
        drop(observation);
        std::fs::remove_dir_all(directory).unwrap();
    }
}

#[cfg(test)]
mod receipt_publication_tests {
    use super::*;

    fn operations(session: &ProjectSession) -> ResolvedProjectAnalysisOperations<'_> {
        let replays = ReplayDeclarations::capture(&session.project);
        let (pipeline_inputs, pipeline_input_error, output_catalog) =
            pipeline_input_observation(session, &replays);
        assert!(pipeline_input_error.is_none(), "{pipeline_input_error:?}");
        ResolvedProjectAnalysisOperations {
            session,
            cache: ProjectAnalysisCache::deferred(&session.manifest),
            check: false,
            functions: None,
            interfaces: None,
            planner: None,
            pipeline_inputs,
            pipeline_input_error,
            output_catalog,
            replays,
            completed_outputs: Vec::new(),
            coverage_report: None,
            captures: std::sync::OnceLock::new(),
        }
    }

    fn active_epoch(manifest: &std::path::Path) -> String {
        let path = manifest
            .parent()
            .unwrap()
            .join("generated/.blobray-cache/queries.sqlite3");
        let connection =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        connection
            .query_row(
                "SELECT active_epoch FROM cache_state WHERE singleton = 1",
                [],
                |row| row.get(0),
            )
            .unwrap()
    }

    #[test]
    fn coordinator_keeps_previous_epoch_when_a_completed_cached_output_changes() {
        let directory = tempfile::tempdir().unwrap();
        let target = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/generic-project/target.toml");
        let manifest = directory.path().join("project.toml");
        std::fs::write(&manifest, format!("schema = 4\nid = \"receipt-fixture\"\ntarget-spec = {:?}\n[analysis.symbols]\noutput = \"generated/result\"\n", target.display().to_string())).unwrap();
        let session = ProjectSession::open_with(&manifest, Default::default()).unwrap();
        let output = session
            .project
            .symbol_inventory
            .as_ref()
            .unwrap()
            .output
            .clone();
        {
            let mut first = operations(&session);
            let work = first
                .resolve_work(
                    "symbol-inventory",
                    false,
                    vec![manifest.clone()],
                    vec![output.clone()],
                )
                .unwrap();
            let WorkDecision::Execute(execution) = first.prepare_work(work).unwrap() else {
                panic!("cold work must execute")
            };
            execution
                .outputs()
                .file(0, "fixture")
                .unwrap()
                .text("original")
                .unwrap();
            first.complete_work(execution).unwrap();
            first.complete_analysis_epoch().unwrap();
        }
        let previous = active_epoch(&manifest);
        let mut next = operations(&session);
        let work = next
            .resolve_work(
                "symbol-inventory",
                false,
                vec![manifest.clone()],
                vec![output.clone()],
            )
            .unwrap();
        assert!(matches!(
            next.prepare_work(work).unwrap(),
            WorkDecision::Complete(StageRun::Current)
        ));
        assert_eq!(next.completed_outputs.len(), 1);
        std::fs::write(&output, "modified").unwrap();
        let error = next.complete_analysis_epoch().unwrap_err();
        assert!(error.to_string().contains("changed after emission"));
        assert_eq!(active_epoch(&manifest), previous);
    }
}
