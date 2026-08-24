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
        ProjectRegisterWorkspace, RegisterFacts, RegisterModel, render_register_review,
        validate_pac_api, validate_register_evidence, validate_register_lints,
        validate_register_memory_map,
    },
    run_spec::{InputRole, RunSpec},
};

use super::{
    ProjectAnalysisInputs, ProjectAnalysisOperations, ProjectAnalysisReport,
    ProjectAnalysisRequest, cache::ProjectAnalysisCache,
};
use crate::application::{ProjectSession, pipeline::StageRun};

pub(crate) fn analyze_project(
    session: &ProjectSession,
    request: ProjectAnalysisRequest,
) -> ProjectAnalysisReport {
    let inputs = ProjectAnalysisInputs {
        run_spec: session.run_spec.is_some(),
        memory_map: session.memory_map.is_some(),
        event_replays: session.project.functions.as_ref().is_some_and(|paths| {
            crate::function_workspace::FunctionPack::load_reviewed(&paths.pack)
                .map_or(true, |pack| {
                    pack.event_routes.iter().any(|route| route.replay.is_some())
                })
        }),
    };
    let mut operations = ResolvedProjectAnalysisOperations {
        session,
        cache: if request.check {
            ProjectAnalysisCache::disabled()
        } else {
            ProjectAnalysisCache::load(&session.manifest)
        },
        check: request.check,
        functions: None,
        interfaces: None,
    };
    super::run(&session.project, request, inputs, &mut operations)
}

struct ResolvedProjectAnalysisOperations<'a> {
    session: &'a ProjectSession,
    cache: ProjectAnalysisCache,
    check: bool,
    functions: Option<FunctionWorkspace>,
    interfaces: Option<InterfaceWorkspace>,
}

impl ResolvedProjectAnalysisOperations<'_> {
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
        paths.extend(self.session.svd_paths.iter().cloned());
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
        if let Some(interfaces) = project.interfaces.as_ref() {
            paths.push(interfaces.facts.clone());
            paths.extend(interfaces.pack.iter().cloned());
            paths.extend(interfaces.semantic_catalogs.iter().cloned());
        }
        if let Some(symbols) = project.symbol_inventory.as_ref() {
            paths.push(symbols.output.clone());
        }
        Ok(paths)
    }

    fn common_inputs(&self) -> Vec<std::path::PathBuf> {
        Vec::new()
    }

    fn run_inputs(&self) -> Vec<std::path::PathBuf> {
        let mut paths = self.common_inputs();
        paths.extend(self.session.run_spec_path.iter().cloned());
        if let Some(run_spec) = self.session.run_spec.as_ref() {
            paths.extend(run_spec.inputs().iter().map(|input| input.path.clone()));
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

    fn register_workspace_inputs(&self, include_ir: bool) -> Vec<std::path::PathBuf> {
        let mut paths = self.common_inputs();
        paths.extend(self.session.project.memory_map.iter().cloned());
        if let Some(registers) = self.session.project.registers.as_ref() {
            paths.push(registers.facts.clone());
            paths.push(registers.model.clone());
            paths.extend(registers.api_pack.iter().cloned());
            paths.extend(registers.lint_pack.iter().cloned());
            paths.extend(registers.evidence_catalogs.iter().cloned());
            if include_ir {
                paths.extend(registers.review_ir_reports.iter().cloned());
            }
        }
        paths
    }

    fn cache_hit(
        &mut self,
        stage: &str,
        check: bool,
        inputs: &[std::path::PathBuf],
        outputs: &[std::path::PathBuf],
    ) -> Result<bool> {
        if check {
            return Ok(false);
        }
        let configuration = self.stage_configuration(stage);
        let current = self
            .cache
            .is_current(stage, &configuration, inputs, outputs)?;
        if current {
            tracing::info!(cache_stage = stage, "content-addressed outputs are current");
        }
        Ok(current)
    }

    fn cache_record(
        &mut self,
        stage: &str,
        check: bool,
        inputs: &[std::path::PathBuf],
        outputs: &[std::path::PathBuf],
    ) -> Result<()> {
        if !check {
            let configuration = self.stage_configuration(stage);
            self.cache.record(stage, &configuration, inputs, outputs)?;
        }
        Ok(())
    }

    /// Stable, stage-owned project configuration included in the cache key.
    ///
    /// File contents remain explicit inputs. Keeping unrelated manifest
    /// sections out of this value prevents, for example, a review-scope edit
    /// from invalidating artifact-wide decoding and linked IR.
    fn stage_configuration(&self, stage: &str) -> String {
        let project = &self.session.project;
        if let Some(profile_id) = stage.strip_prefix("linked-ir:") {
            let profile = project
                .ir_profiles
                .iter()
                .find(|profile| profile.id == profile_id)
                .expect("linked-IR cache stage must name a configured profile");
            return format!(
                "sources={:?};roots={:?};include-reachable={};entry-contract={:?}",
                profile.sources, profile.roots, profile.include_reachable, profile.entry_contract
            );
        }
        match stage.split_once(':').map_or(stage, |(owner, _)| owner) {
            "symbol-inventory" => format!("{:?}", project.symbol_inventory),
            "mmio-discovery" => format!(
                "facts={:?}",
                project.registers.as_ref().map(|registers| &registers.facts)
            ),
            "interface-discovery" => format!(
                "facts={:?}",
                project
                    .interfaces
                    .as_ref()
                    .map(|interfaces| &interfaces.facts)
            ),
            "linked-ir" => format!("{:?}", project.ir_profiles),
            "event-replays" => format!("{:?}", project.functions),
            "review-scopes" => format!(
                "review={:?};policy={:?}",
                project.review,
                project
                    .verification
                    .as_ref()
                    .and_then(|verification| verification.policy.as_ref())
            ),
            "navigation-index" => format!("{:?}", project.navigation_index),
            "code-boundary-validation" | "code-boundary-review" => {
                format!("{:?}", project.code)
            }
            "register-validation" | "register-review" => {
                format!("{:?}", project.registers)
            }
            "function-validation" | "function-review" => {
                format!("{:?}", project.functions)
            }
            "interface-validation" => format!("{:?}", project.interfaces),
            _ => unreachable!("cache stage revision rejects unknown stage {stage:?}"),
        }
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

impl ProjectAnalysisOperations for ResolvedProjectAnalysisOperations<'_> {
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
        if self.cache_hit("symbol-inventory", check, &inputs, &outputs)? {
            return Ok(StageRun::Current);
        }
        build_symbol_inventory(&self.session.project, self.run_spec()?, check)?;
        self.cache_record("symbol-inventory", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn discover_mmio(&mut self, check: bool, jobs: usize) -> Result<StageRun> {
        let mut inputs = self.run_inputs();
        inputs.extend(self.session.project.memory_map.iter().cloned());
        inputs.extend(self.session.svd_paths.iter().cloned());
        if let Some(code) = self.session.project.code.as_ref() {
            inputs.push(code.pack.clone());
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
        if self.cache_hit("mmio-discovery", check, &inputs, &outputs)? {
            return Ok(StageRun::Current);
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
        let mut inputs = self.run_inputs();
        if let Some(code) = self.session.project.code.as_ref() {
            inputs.push(code.pack.clone());
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
        if self.cache_hit("interface-discovery", check, &inputs, &outputs)? {
            return Ok(StageRun::Current);
        }
        discover_project_interfaces_operation(&self.session.project, self.run_spec()?, check)?;
        self.cache_record("interface-discovery", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn build_linked_ir(&mut self, check: bool, jobs: usize) -> Result<StageRun> {
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
        for profile in profiles {
            let stage = format!("linked-ir:{}", profile.id);
            let inputs = self.linked_ir_inputs(&profile)?;
            let outputs = self.linked_ir_outputs(&profile);
            if self.cache_hit(&stage, false, &inputs, &outputs)? {
                continue;
            }
            stale.push((profile.id, stage, inputs, outputs));
        }
        if stale.is_empty() {
            return Ok(StageRun::Current);
        }

        crate::application::project_ir_build::build_project_ir(
            crate::application::project_ir_build::ProjectIrBuildRequest {
                profiles: stale
                    .iter()
                    .map(|(profile, _, _, _)| profile.clone())
                    .collect(),
                check: false,
                jobs,
                refresh_review_scopes: false,
            },
            &self.session.manifest,
            &self.session.project,
            self.run_spec()?,
            &self.session.mmio,
            &self.session.target,
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
                crate::application::event_replay::EventReplayRequest {
                    manifest: replay.manifest,
                    artifact,
                    companion,
                },
                replay.evidence,
            ));
        }

        let mut inputs = self.target_inputs();
        inputs.push(functions.pack.clone());
        inputs.extend(self.session.project.memory_map.iter().cloned());
        inputs.extend(self.session.svd_paths.iter().cloned());
        for (request, _) in &requests {
            inputs.push(request.manifest.clone());
            inputs.push(request.artifact.clone());
            inputs.extend(request.companion.iter().cloned());
        }
        if let Some(interfaces) = self.session.project.interfaces.as_ref() {
            inputs.push(interfaces.facts.clone());
            inputs.extend(interfaces.pack.iter().cloned());
            inputs.extend(interfaces.semantic_catalogs.iter().cloned());
        }
        let outputs = requests
            .iter()
            .map(|(_, output)| output.clone())
            .collect::<Vec<_>>();
        if self.cache_hit("event-replays", check, &inputs, &outputs)? {
            return Ok(StageRun::Current);
        }
        for (request, output) in requests {
            let document = crate::application::event_replay::execute(
                &request,
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
        if self.cache_hit("review-scopes", check, &inputs, &outputs)? {
            return Ok(StageRun::Current);
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
        if self.cache_hit("navigation-index", check, &inputs, &outputs)? {
            return Ok(StageRun::Current);
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
        if self.cache_hit(&stage, self.check, &inputs, &[])? {
            return Ok(StageRun::Current);
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
        if self.cache_hit("code-boundary-review", check, &inputs, &outputs)? {
            return Ok(StageRun::Current);
        }
        successful(review_code_boundaries(&self.session.project, check)?)?;
        self.cache_record("code-boundary-review", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn validate_registers(&mut self, deny_unreviewed: bool) -> Result<StageRun> {
        let inputs = self.register_workspace_inputs(false);
        let stage = validation_key("register-validation", deny_unreviewed);
        if self.cache_hit(&stage, self.check, &inputs, &[])? {
            return Ok(StageRun::Current);
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
        let inputs = self.register_workspace_inputs(true);
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
        if self.cache_hit("register-review", check, &inputs, &outputs)? {
            return Ok(StageRun::Current);
        }
        successful(review_registers(&self.session.project, check)?)?;
        self.cache_record("register-review", check, &inputs, &outputs)?;
        Ok(StageRun::Executed)
    }

    fn validate_functions(&mut self, deny_unreviewed: bool) -> Result<StageRun> {
        let inputs = self.function_workspace_inputs()?;
        let stage = validation_key("function-validation", deny_unreviewed);
        if self.cache_hit(&stage, self.check, &inputs, &[])? {
            return Ok(StageRun::Current);
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
        inputs.extend(self.interface_workspace_inputs());
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
        if self.cache_hit("function-review", check, &inputs, &outputs)? {
            self.functions = None;
            return Ok(StageRun::Current);
        }
        self.ensure_function_workspace()?;
        let has_interface_pack = self
            .session
            .project
            .interfaces
            .as_ref()
            .and_then(|paths| paths.pack.as_deref())
            .is_some_and(std::path::Path::is_file);
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
        if self.cache_hit(&stage, self.check, &inputs, &[])? {
            return Ok(StageRun::Current);
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
    let model = RegisterModel::load(&paths.model)?;
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
