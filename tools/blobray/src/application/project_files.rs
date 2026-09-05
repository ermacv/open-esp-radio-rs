//! Typed inventory of project inputs, reviewed knowledge and generated outputs.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::{FollowUpStep, ProjectContext, ProjectContextRequirement};
use crate::{Result, run_spec::RunSpec};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ProjectFileOwnership {
    Entrypoint,
    Local,
    External,
    Reviewed,
    Generated,
}

/// Portability/knowledge boundary, independent from who edits the file.
///
/// For example, a chip register model and a project disposition are both
/// reviewed inputs, but only the first is reusable across investigations.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ProjectFileLayer {
    Composition,
    Architecture,
    Ecosystem,
    Chip,
    Investigation,
    LocalBinding,
    ExternalArtifact,
    Generated,
}

impl ProjectFileLayer {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Composition => "composition",
            Self::Architecture => "architecture",
            Self::Ecosystem => "ecosystem",
            Self::Chip => "chip",
            Self::Investigation => "investigation",
            Self::LocalBinding => "local-binding",
            Self::ExternalArtifact => "external-artifact",
            Self::Generated => "generated",
        }
    }
}

impl ProjectFileOwnership {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Entrypoint => "entrypoint",
            Self::Local => "local",
            Self::External => "external",
            Self::Reviewed => "reviewed",
            Self::Generated => "generated",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ProjectFileState {
    Present,
    Missing,
    Pending,
}

impl ProjectFileState {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Present => "present",
            Self::Missing => "missing",
            Self::Pending => "pending",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ProjectFileEntry {
    pub(crate) role: String,
    pub(crate) ownership: ProjectFileOwnership,
    pub(crate) layer: ProjectFileLayer,
    pub(crate) state: ProjectFileState,
    pub(crate) path: PathBuf,
    pub(crate) producer: Option<String>,
    pub(crate) consumers: Vec<String>,
    pub(crate) required: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) next_step: Option<FollowUpStep>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct ProjectFilesReport {
    pub(crate) schema: u32,
    pub(crate) project_id: String,
    pub(crate) manifest: PathBuf,
    pub(crate) files: Vec<ProjectFileEntry>,
    pub(crate) present: usize,
    pub(crate) missing: usize,
    pub(crate) pending: usize,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ProjectFilesWorkflowState {
    Blocked,
    AnalysisPending,
    ReviewOutputsPending,
    ReviewConfigurationRequired,
    PublicationPreflightRequired,
    VerificationPending,
    FilesPresent,
}

impl ProjectFilesReport {
    pub(crate) fn workflow_state(&self) -> ProjectFilesWorkflowState {
        if self.required_missing() != 0 {
            return ProjectFilesWorkflowState::Blocked;
        }
        if self.pending_generated_by(|producer| producer == "project analyze") {
            return ProjectFilesWorkflowState::AnalysisPending;
        }
        if self.pending_generated_by(is_review_producer) {
            return ProjectFilesWorkflowState::ReviewOutputsPending;
        }
        if self.pending_generated_by(|producer| producer == "project publish") {
            if self.file("review-workspace").is_none() {
                return ProjectFilesWorkflowState::ReviewConfigurationRequired;
            }
            return ProjectFilesWorkflowState::PublicationPreflightRequired;
        }
        if self.pending_generated_by(|producer| producer == "project verify") {
            return ProjectFilesWorkflowState::VerificationPending;
        }
        ProjectFilesWorkflowState::FilesPresent
    }

    pub(crate) fn required_missing(&self) -> usize {
        self.files
            .iter()
            .filter(|file| file.required && file.state == ProjectFileState::Missing)
            .count()
    }

    pub(crate) fn pending_analysis_outputs(&self) -> usize {
        self.pending_generated_count_by(|producer| producer == "project analyze")
    }

    pub(crate) fn pending_review_outputs(&self) -> usize {
        self.pending_generated_count_by(is_review_producer)
    }

    pub(crate) fn pending_verification_outputs(&self) -> usize {
        self.pending_generated_count_by(|producer| producer == "project verify")
    }

    /// Return only the next executable commands or manual actions whose prerequisites are
    /// present. The file inventory deliberately does not recommend every
    /// producer at once: later bootstrap and publication stages would fail and
    /// hide the actual first blocker.
    pub(crate) fn next_steps(&self, context: &ProjectContext<'_>) -> Result<Vec<FollowUpStep>> {
        if self.is_missing("run-spec") {
            return Ok(vec![FollowUpStep::command(
                "Initialize the local input bindings.",
                context.inputs_init_help_action()?,
            )]);
        }

        let missing_inputs = self
            .files
            .iter()
            .filter(|file| {
                file.ownership == ProjectFileOwnership::External
                    && file.state == ProjectFileState::Missing
            })
            .filter_map(|file| file.next_step.clone())
            .collect::<Vec<_>>();
        if !missing_inputs.is_empty() {
            return Ok(missing_inputs);
        }

        if self.is_missing("code-pack") {
            return Ok(vec![if self.is_present("symbol-inventory") {
                executable_step(
                    context,
                    "Create the reviewed code pack from the current symbol inventory.",
                    ["advanced", "code", "init-pack"],
                    ProjectContextRequirement::ProjectOnly,
                )?
            } else {
                executable_step(
                    context,
                    "Generate the symbol inventory required by the code pack.",
                    ["advanced", "symbols", "inventory"],
                    ProjectContextRequirement::RunSpec,
                )?
            }]);
        }
        if self.is_missing("interface-pack") {
            return Ok(vec![if self.is_present("interface-facts") {
                executable_step(
                    context,
                    "Create the reviewed interface pack from the discovered facts.",
                    ["advanced", "interfaces", "init-pack"],
                    ProjectContextRequirement::Target,
                )?
            } else {
                executable_step(
                    context,
                    "Discover interface facts required by the interface pack.",
                    ["advanced", "interfaces", "discover"],
                    ProjectContextRequirement::RunSpec,
                )?
            }]);
        }
        if self.is_missing("function-pack") {
            let linked_ir_ready = self
                .files
                .iter()
                .filter(|file| file.role.starts_with("linked-ir:"))
                .all(|file| file.state == ProjectFileState::Present);
            return Ok(vec![if linked_ir_ready {
                executable_step(
                    context,
                    "Create the reviewed function pack from linked IR.",
                    ["advanced", "functions", "init-pack"],
                    ProjectContextRequirement::Target,
                )?
            } else {
                executable_step(
                    context,
                    "Build the linked IR required by the function pack.",
                    ["advanced", "ir", "build"],
                    ProjectContextRequirement::Analysis,
                )?
            }]);
        }

        if self.required_missing() != 0 {
            return Ok(vec![executable_step(
                context,
                "Diagnose the remaining required project files.",
                ["project", "doctor"],
                ProjectContextRequirement::Analysis,
            )?]);
        }

        let steps = match self.workflow_state() {
            ProjectFilesWorkflowState::Blocked => vec![executable_step(
                context,
                "Diagnose the blocked project workflow.",
                ["project", "doctor"],
                ProjectContextRequirement::Analysis,
            )?],
            ProjectFilesWorkflowState::AnalysisPending => vec![executable_step(
                context,
                "Generate the pending analysis outputs.",
                ["project", "analyze"],
                ProjectContextRequirement::Analysis,
            )?],
            ProjectFilesWorkflowState::ReviewOutputsPending => [
                (
                    "advanced code review",
                    ["advanced", "code", "review"].as_slice(),
                    ProjectContextRequirement::ProjectOnly,
                ),
                (
                    "registers review",
                    ["registers", "review"].as_slice(),
                    ProjectContextRequirement::ProjectOnly,
                ),
                (
                    "advanced functions review",
                    ["advanced", "functions", "review"].as_slice(),
                    ProjectContextRequirement::Target,
                ),
            ]
            .into_iter()
            .filter(|(producer, _, _)| self.pending_generated_by(|pending| pending == *producer))
            .map(|(producer, command, requirement)| {
                executable_step(
                    context,
                    format!("Generate the pending `{producer}` review output."),
                    command.iter().copied(),
                    requirement,
                )
            })
            .collect::<Result<Vec<_>>>()?,
            ProjectFilesWorkflowState::ReviewConfigurationRequired => {
                vec![FollowUpStep::manual(format!(
                    "Configure [review] and publication-scopes in {}.",
                    self.manifest.display()
                ))]
            }
            ProjectFilesWorkflowState::PublicationPreflightRequired => vec![executable_step(
                context,
                "Validate the generated publication outputs without modifying them.",
                ["project", "publish", "--check"],
                ProjectContextRequirement::ProjectOnly,
            )?],
            ProjectFilesWorkflowState::VerificationPending => vec![executable_step(
                context,
                "Generate the pending verification outputs.",
                ["project", "verify"],
                ProjectContextRequirement::Analysis,
            )?],
            ProjectFilesWorkflowState::FilesPresent => vec![executable_step(
                context,
                "Inspect project readiness and the next research frontier.",
                ["project", "status"],
                ProjectContextRequirement::RunSpec,
            )?],
        };
        Ok(steps)
    }

    fn file(&self, role: &str) -> Option<&ProjectFileEntry> {
        self.files.iter().find(|file| file.role == role)
    }

    fn is_missing(&self, role: &str) -> bool {
        self.file(role)
            .is_some_and(|file| file.state == ProjectFileState::Missing)
    }

    fn is_present(&self, role: &str) -> bool {
        self.file(role)
            .is_some_and(|file| file.state == ProjectFileState::Present)
    }

    fn pending_generated_by(&self, predicate: impl Fn(&str) -> bool) -> bool {
        self.pending_generated_count_by(predicate) != 0
    }

    fn pending_generated_count_by(&self, predicate: impl Fn(&str) -> bool) -> usize {
        self.files
            .iter()
            .filter(|file| {
                file.state == ProjectFileState::Pending
                    && file.producer.as_deref().is_some_and(&predicate)
            })
            .count()
    }
}

fn executable_step<I, S>(
    context: &ProjectContext<'_>,
    instruction: impl Into<String>,
    command: I,
    requirement: ProjectContextRequirement,
) -> Result<FollowUpStep>
where
    I: IntoIterator<Item = S>,
    S: Into<String>,
{
    Ok(FollowUpStep::command(
        instruction,
        context.follow_up_action(command, requirement)?,
    ))
}

fn is_review_producer(producer: &str) -> bool {
    matches!(
        producer,
        "advanced code review" | "registers review" | "advanced functions review"
    )
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ProjectOwnershipReport {
    pub(crate) unowned_reviewed: Vec<PathBuf>,
    pub(crate) stale_generated: Vec<PathBuf>,
}

impl ProjectOwnershipReport {
    pub(crate) fn passed(&self) -> bool {
        self.unowned_reviewed.is_empty() && self.stale_generated.is_empty()
    }

    pub(crate) fn issue_count(&self) -> usize {
        self.unowned_reviewed.len() + self.stale_generated.len()
    }
}

/// Find files that survive after their owning verification declaration has
/// been removed. Only declaration-owned leaf directories are scanned: linked
/// IR directories contain internal indexes and are deliberately outside this
/// check.
pub(crate) fn collect_ownership(project: &crate::ProjectSpec) -> Result<ProjectOwnershipReport> {
    let Some(verification) = project.verification.as_ref() else {
        return Ok(ProjectOwnershipReport {
            unowned_reviewed: Vec::new(),
            stale_generated: Vec::new(),
        });
    };

    let mut reviewed = BTreeMap::<PathBuf, BTreeSet<PathBuf>>::new();
    for suite in &verification.suites {
        for paths in [
            &suite.profiles,
            &suite.dispositions,
            &suite.evidence_baselines,
        ] {
            for path in paths {
                let Some(parent) = path.parent() else {
                    continue;
                };
                reviewed
                    .entry(parent.to_owned())
                    .or_default()
                    .insert(path.to_owned());
            }
        }
    }
    let unowned_reviewed = unowned_files(&reviewed, Some("toml"))?;

    let suite_report_directory = verification
        .report
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("verification-suites");
    let expected_suite_reports = verification
        .suites
        .iter()
        .map(|suite| suite_report_directory.join(format!("{}.json", suite.id)))
        .collect::<BTreeSet<_>>();
    let mut generated = BTreeMap::new();
    generated.insert(suite_report_directory, expected_suite_reports);

    let report_directory = verification
        .report
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .to_owned();
    let mut expected_reports = BTreeSet::from([verification.report.clone()]);
    for output in [
        project
            .code
            .as_ref()
            .and_then(|paths| paths.review_output.as_ref()),
        project
            .registers
            .as_ref()
            .and_then(|paths| paths.review_output.as_ref()),
        project
            .functions
            .as_ref()
            .and_then(|paths| paths.review_output.as_ref()),
    ]
    .into_iter()
    .flatten()
    {
        if output.parent() == Some(report_directory.as_path()) {
            expected_reports.insert(output.clone());
        }
    }
    generated.insert(report_directory, expected_reports);
    let stale_generated = unowned_files(&generated, None)?;

    Ok(ProjectOwnershipReport {
        unowned_reviewed,
        stale_generated,
    })
}

fn unowned_files(
    directories: &BTreeMap<PathBuf, BTreeSet<PathBuf>>,
    extension: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let mut unowned = Vec::new();
    for (directory, expected) in directories {
        if !directory.is_dir() {
            continue;
        }
        for entry in fs::read_dir(directory)? {
            let path = entry?.path();
            if !path.is_file()
                || extension.is_some_and(|extension| {
                    path.extension().and_then(|value| value.to_str()) != Some(extension)
                })
            {
                continue;
            }
            if !expected.contains(&path) {
                unowned.push(path);
            }
        }
    }
    unowned.sort();
    unowned.dedup();
    Ok(unowned)
}

pub(crate) fn collect(context: &ProjectContext<'_>) -> Result<ProjectFilesReport> {
    let project = context.project;
    let mut files = Vec::new();
    push(
        &mut files,
        "project-manifest",
        ProjectFileOwnership::Entrypoint,
        context.project_path,
        None,
        &["all project workflows"],
        true,
        None,
    );
    push(
        &mut files,
        "target-spec",
        ProjectFileOwnership::Reviewed,
        context.target_path,
        None,
        &["backend selection", "code generation"],
        true,
        None,
    );
    for (pack_index, pack) in project.ecosystem_packs.iter().enumerate() {
        push(
            &mut files,
            format!("ecosystem-pack[{pack_index}]"),
            ProjectFileOwnership::Reviewed,
            &pack.path,
            None,
            &["semantic analysis", "interface enrichment"],
            false,
            None,
        );
        for (index, path) in pack.knowledge_packs.iter().enumerate() {
            push(
                &mut files,
                format!("ecosystem-pack[{pack_index}].knowledge[{index}]"),
                ProjectFileOwnership::Reviewed,
                path,
                None,
                &["linked IR", "interface semantics"],
                false,
                None,
            );
        }
        for (index, path) in pack.capability_packs.iter().enumerate() {
            push(
                &mut files,
                format!("ecosystem-pack[{pack_index}].capability[{index}]"),
                ProjectFileOwnership::Reviewed,
                path,
                None,
                &["interface capability report", "linked IR"],
                false,
                None,
            );
        }
        for (index, path) in pack.interface_template_packs.iter().enumerate() {
            push(
                &mut files,
                format!("ecosystem-pack[{pack_index}].interface-template[{index}]"),
                ProjectFileOwnership::Reviewed,
                path,
                None,
                &["interface layout composition", "linked IR"],
                false,
                None,
            );
        }
    }
    if let Some(pack) = &project.chip_pack {
        push(
            &mut files,
            "chip-pack",
            ProjectFileOwnership::Reviewed,
            &pack.path,
            None,
            &[
                "chip addresses",
                "register catalogs",
                "compiled chip knowledge",
            ],
            true,
            None,
        );
        for (index, path) in pack.knowledge_packs.iter().enumerate() {
            push(
                &mut files,
                format!("chip-pack.knowledge[{index}]"),
                ProjectFileOwnership::Reviewed,
                path,
                None,
                &["linked IR", "chip semantics"],
                false,
                None,
            );
        }
        for (index, path) in pack.capability_packs.iter().enumerate() {
            push(
                &mut files,
                format!("chip-pack.capability[{index}]"),
                ProjectFileOwnership::Reviewed,
                path,
                None,
                &["interface capability report", "linked IR"],
                false,
                None,
            );
        }
        for (index, path) in pack.interface_template_packs.iter().enumerate() {
            push(
                &mut files,
                format!("chip-pack.interface-template[{index}]"),
                ProjectFileOwnership::Reviewed,
                path,
                None,
                &["interface layout composition", "linked IR"],
                false,
                None,
            );
        }
    }
    if let Some(path) = project.memory_map.as_deref() {
        push(
            &mut files,
            "memory-map",
            ProjectFileOwnership::Reviewed,
            path,
            None,
            &["MMIO classification", "artifact analysis"],
            true,
            None,
        );
    }
    for (index, path) in context.svd_paths.iter().enumerate() {
        push(
            &mut files,
            format!("svd-catalog[{index}]"),
            ProjectFileOwnership::Reviewed,
            path,
            None,
            &["register naming", "MMIO enrichment"],
            false,
            None,
        );
    }
    for (index, path) in project.reviewed_knowledge.iter().enumerate() {
        push(
            &mut files,
            format!("reviewed-knowledge[{index}]"),
            ProjectFileOwnership::Reviewed,
            path,
            None,
            &[
                "effective register model",
                "SVD/PAC publication",
                "revision rebase",
            ],
            true,
            None,
        );
    }

    let run_spec_path = configured_run_spec(context.project_path, project.run_spec.as_deref());
    let initialize_inputs = FollowUpStep::command(
        "Initialize the local input bindings.",
        context.inputs_init_help_action()?,
    );
    push(
        &mut files,
        "run-spec",
        ProjectFileOwnership::Local,
        &run_spec_path,
        Some("project inputs init"),
        &["artifact bindings", "verification"],
        true,
        Some(initialize_inputs),
    );
    if run_spec_path.is_file() {
        for input in RunSpec::load(&run_spec_path)?.inputs() {
            let repair_instruction = format!(
                "rebuild or restore {}, or update its binding in {}",
                input.path.display(),
                run_spec_path.display()
            );
            push(
                &mut files,
                format!("input:{}", input.role),
                ProjectFileOwnership::External,
                &input.path,
                None,
                &["analysis pipeline"],
                true,
                Some(FollowUpStep::manual(repair_instruction)),
            );
        }
    }

    if let Some(symbols) = &project.symbol_inventory {
        push_generated(
            &mut files,
            "symbol-inventory",
            &symbols.output,
            "project analyze",
            &["code boundaries", "navigation", "interfaces"],
        );
    }
    if let Some(navigation) = &project.navigation_index {
        push_generated(
            &mut files,
            "navigation-index",
            &navigation.output,
            "project analyze",
            &["inspect", "TUI"],
        );
    }
    if let Some(code) = &project.code {
        push_reviewed(&mut files, "code-pack", &code.pack, &["linked IR"]);
        if let Some(path) = &code.review_output {
            push_generated(
                &mut files,
                "code-review",
                path,
                "advanced code review",
                &["human review"],
            );
        }
    }
    for profile in &project.ir_profiles {
        push_generated(
            &mut files,
            format!("linked-ir:{}", profile.id),
            &profile.output,
            "project analyze",
            &["function review", "register review", "inspect"],
        );
    }
    if let Some(registers) = &project.registers {
        push_generated(
            &mut files,
            "register-facts",
            &registers.facts,
            "project analyze",
            &["register review"],
        );
        push_reviewed(
            &mut files,
            "register-model",
            &registers.model,
            &["SVD publication", "PAC generation"],
        );
        if let Some(path) = &registers.review_output {
            push_generated(
                &mut files,
                "register-review",
                path,
                "registers review",
                &["human review"],
            );
        }
        if let Some(path) = &registers.svd_output {
            push_generated(
                &mut files,
                "published-svd",
                path,
                "project publish",
                &["external tooling"],
            );
        }
        if let Some(spec) = &registers.pac_raw {
            push_generated(
                &mut files,
                "raw-pac",
                &spec.output,
                "project publish",
                &["restricted bindings"],
            );
        }
        if let Some(spec) = &registers.bindings {
            push_generated(
                &mut files,
                "register-bindings",
                &spec.output,
                "project publish",
                &["Rust HAL"],
            );
        }
        for (role, path) in [
            (
                "register-ownership-policy",
                registers.ownership_policy.as_ref(),
            ),
            ("register-api-pack", registers.api_pack.as_ref()),
            ("register-lint-pack", registers.lint_pack.as_ref()),
        ] {
            if let Some(path) = path {
                push_reviewed(&mut files, role, path, &["restricted bindings"]);
            }
        }
        if let Some(path) = &registers.api_output {
            push_generated(
                &mut files,
                "register-api",
                path,
                "project publish",
                &["Rust HAL"],
            );
        }
        for (index, path) in registers.evidence_catalogs.iter().enumerate() {
            push_reviewed(
                &mut files,
                format!("register-evidence[{index}]"),
                path,
                &["register validation"],
            );
        }
    }
    if let Some(interfaces) = &project.interfaces {
        push_generated(
            &mut files,
            "interface-facts",
            &interfaces.facts,
            "project analyze",
            &["interface review", "linked IR"],
        );
        if let Some(path) = &interfaces.pack {
            push_reviewed(&mut files, "interface-pack", path, &["linked IR"]);
        }
        if let Some(path) = &interfaces.capability_context {
            push_generated(
                &mut files,
                "interface-capability-context",
                path,
                "project analyze",
                &["research next"],
            );
        }
        for (index, path) in interfaces.semantic_catalogs.iter().enumerate() {
            push_reviewed(
                &mut files,
                format!("interface-semantics[{index}]"),
                path,
                &["linked IR"],
            );
        }
    }
    if let Some(functions) = &project.functions {
        push_reviewed(
            &mut files,
            "function-pack",
            &functions.pack,
            &["function review", "inspect"],
        );
        if let Some(path) = &functions.review_output {
            push_generated(
                &mut files,
                "function-review",
                path,
                "advanced functions review",
                &["human review"],
            );
        }
    }
    if let Some(review) = &project.review {
        push_generated(
            &mut files,
            "review-workspace",
            &review.output,
            "project analyze",
            &["manual driver analysis"],
        );
    }
    if let Some(verification) = &project.verification {
        if let Some(policy) = &verification.policy {
            push_reviewed(
                &mut files,
                "verification-policy",
                policy,
                &["project check"],
            );
        }
        push_generated(
            &mut files,
            "verification-report",
            &verification.report,
            "project verify",
            &["project check"],
        );
        push_generated(
            &mut files,
            "vendor-evidence-index",
            &verification.evidence_index,
            "project verify",
            &["repository qualification check"],
        );
        for suite in &verification.suites {
            for (kind, paths) in [
                ("profile", &suite.profiles),
                ("disposition", &suite.dispositions),
                ("baseline", &suite.evidence_baselines),
            ] {
                for (index, path) in paths.iter().enumerate() {
                    push_reviewed(
                        &mut files,
                        format!("verification:{}:{kind}[{index}]", suite.id),
                        path,
                        &["project verify"],
                    );
                }
            }
        }
    }

    let ecosystem_resources = project
        .ecosystem_packs
        .iter()
        .flat_map(|pack| {
            pack.knowledge_packs
                .iter()
                .chain(&pack.capability_packs)
                .chain(&pack.interface_template_packs)
        })
        .collect::<BTreeSet<_>>();
    let chip_resources = project
        .chip_pack
        .iter()
        .flat_map(|pack| {
            pack.memory_map
                .iter()
                .chain(&pack.svd_paths)
                .chain(pack.register_model.iter())
                .chain(&pack.knowledge_packs)
                .chain(&pack.capability_packs)
                .chain(&pack.interface_template_packs)
        })
        .collect::<BTreeSet<_>>();
    for file in &mut files {
        if file.ownership == ProjectFileOwnership::Reviewed {
            if ecosystem_resources.contains(&file.path) {
                file.layer = ProjectFileLayer::Ecosystem;
            }
            if chip_resources.contains(&file.path) {
                file.layer = ProjectFileLayer::Chip;
            }
        }
    }

    files.sort_by(|left, right| {
        (
            left.layer,
            left.ownership,
            left.role.as_str(),
            left.path.as_os_str(),
        )
            .cmp(&(
                right.layer,
                right.ownership,
                right.role.as_str(),
                right.path.as_os_str(),
            ))
    });
    let present = files
        .iter()
        .filter(|file| file.state == ProjectFileState::Present)
        .count();
    let missing = files
        .iter()
        .filter(|file| file.state == ProjectFileState::Missing)
        .count();
    let pending = files
        .iter()
        .filter(|file| file.state == ProjectFileState::Pending)
        .count();
    Ok(ProjectFilesReport {
        schema: 4,
        project_id: project.id.clone(),
        manifest: context.project_path.to_owned(),
        files,
        present,
        missing,
        pending,
    })
}

fn configured_run_spec(manifest: &Path, configured: Option<&Path>) -> PathBuf {
    configured.map(Path::to_owned).unwrap_or_else(|| {
        manifest
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("local.toml")
    })
}

fn push_generated(
    files: &mut Vec<ProjectFileEntry>,
    role: impl Into<String>,
    path: &Path,
    producer: &str,
    consumers: &[&str],
) {
    push(
        files,
        role,
        ProjectFileOwnership::Generated,
        path,
        Some(producer),
        consumers,
        false,
        None,
    );
}

fn push_reviewed(
    files: &mut Vec<ProjectFileEntry>,
    role: impl Into<String>,
    path: &Path,
    consumers: &[&str],
) {
    push(
        files,
        role,
        ProjectFileOwnership::Reviewed,
        path,
        None,
        consumers,
        true,
        None,
    );
}

#[allow(clippy::too_many_arguments)]
fn push(
    files: &mut Vec<ProjectFileEntry>,
    role: impl Into<String>,
    ownership: ProjectFileOwnership,
    path: &Path,
    producer: Option<&str>,
    consumers: &[&str],
    required: bool,
    next_step: Option<FollowUpStep>,
) {
    let role = role.into();
    let state = if path.exists() {
        ProjectFileState::Present
    } else if ownership == ProjectFileOwnership::Generated {
        ProjectFileState::Pending
    } else {
        ProjectFileState::Missing
    };
    files.push(ProjectFileEntry {
        layer: default_layer(&role, ownership),
        role,
        ownership,
        state,
        path: path.to_owned(),
        producer: producer.map(str::to_owned),
        consumers: consumers.iter().map(|value| (*value).to_owned()).collect(),
        required,
        next_step: (state != ProjectFileState::Present)
            .then_some(next_step)
            .flatten(),
    });
}

fn default_layer(role: &str, ownership: ProjectFileOwnership) -> ProjectFileLayer {
    match ownership {
        ProjectFileOwnership::Entrypoint => ProjectFileLayer::Composition,
        ProjectFileOwnership::Local => ProjectFileLayer::LocalBinding,
        ProjectFileOwnership::External => ProjectFileLayer::ExternalArtifact,
        ProjectFileOwnership::Generated => ProjectFileLayer::Generated,
        ProjectFileOwnership::Reviewed if role == "target-spec" => ProjectFileLayer::Architecture,
        ProjectFileOwnership::Reviewed if role.starts_with("ecosystem-pack") => {
            ProjectFileLayer::Ecosystem
        }
        ProjectFileOwnership::Reviewed if role.starts_with("chip-pack") => ProjectFileLayer::Chip,
        ProjectFileOwnership::Reviewed => ProjectFileLayer::Investigation,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{MmioMap, TargetSpec, application::ExplicitProjectContext, project::ProjectSpec};

    fn next_steps(report: &ProjectFilesReport) -> Vec<FollowUpStep> {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/generic-project/vendor-project.toml");
        let project = ProjectSpec::load(&manifest).unwrap();
        let target = TargetSpec::load(&project.target_spec).unwrap();
        let svd = MmioMap::load_all(&[]).unwrap();
        let explicit_context = ExplicitProjectContext::default();
        let invocation_directory = std::env::current_dir().unwrap();
        let context = ProjectContext {
            project_path: &manifest,
            project: &project,
            target_path: &project.target_spec,
            target: &target,
            run_spec_path: None,
            run_spec: None,
            memory_map: None,
            svd_paths: &[],
            svd: &svd,
            explicit_context: &explicit_context,
            invocation_directory: &invocation_directory,
        };
        report.next_steps(&context).unwrap()
    }

    fn next_commands(report: &ProjectFilesReport) -> Vec<Vec<String>> {
        next_steps(report)
            .into_iter()
            .flat_map(|step| step.commands)
            .map(|action| {
                let mut command = Vec::new();
                let mut arguments = action.argv[1..].iter();
                while let Some(argument) = arguments.next() {
                    if matches!(
                        argument.as_str(),
                        "--project" | "--target-spec" | "--run-spec" | "--svd"
                    ) {
                        let _ = arguments.next();
                    } else {
                        command.push(argument.clone());
                    }
                }
                command
            })
            .collect()
    }

    fn entry(
        role: &str,
        ownership: ProjectFileOwnership,
        state: ProjectFileState,
        producer: Option<&str>,
        required: bool,
    ) -> ProjectFileEntry {
        ProjectFileEntry {
            role: role.to_owned(),
            ownership,
            layer: default_layer(role, ownership),
            state,
            path: PathBuf::from(role),
            producer: producer.map(str::to_owned),
            consumers: Vec::new(),
            required,
            next_step: None,
        }
    }

    fn report(files: Vec<ProjectFileEntry>) -> ProjectFilesReport {
        ProjectFilesReport {
            schema: 4,
            project_id: "fixture".to_owned(),
            manifest: PathBuf::from("vendor-project.toml"),
            present: files
                .iter()
                .filter(|file| file.state == ProjectFileState::Present)
                .count(),
            missing: files
                .iter()
                .filter(|file| file.state == ProjectFileState::Missing)
                .count(),
            pending: files
                .iter()
                .filter(|file| file.state == ProjectFileState::Pending)
                .count(),
            files,
        }
    }

    #[test]
    fn ownership_scan_rejects_only_unreferenced_leaf_files() {
        let directory =
            std::env::temp_dir().join(format!("blobray-file-ownership-{}", std::process::id()));
        if directory.exists() {
            fs::remove_dir_all(&directory).unwrap();
        }
        fs::create_dir_all(&directory).unwrap();
        let owned = directory.join("owned.toml");
        let stale = directory.join("stale.toml");
        let unrelated = directory.join("notes.md");
        fs::write(&owned, "schema = 3\n").unwrap();
        fs::write(&stale, "schema = 3\n").unwrap();
        fs::write(&unrelated, "review notes\n").unwrap();

        let directories = BTreeMap::from([(directory.clone(), BTreeSet::from([owned]))]);
        assert_eq!(
            unowned_files(&directories, Some("toml")).unwrap(),
            vec![stale]
        );

        fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn file_layers_separate_portability_from_review_ownership() {
        assert_eq!(
            default_layer(
                "ecosystem-pack[0].knowledge[0]",
                ProjectFileOwnership::Reviewed
            ),
            ProjectFileLayer::Ecosystem
        );
        assert_eq!(
            default_layer(
                "ecosystem-pack[0].capability[0]",
                ProjectFileOwnership::Reviewed
            ),
            ProjectFileLayer::Ecosystem
        );
        assert_eq!(
            default_layer("chip-pack", ProjectFileOwnership::Reviewed),
            ProjectFileLayer::Chip
        );
        assert_eq!(
            default_layer("chip-pack.capability[0]", ProjectFileOwnership::Reviewed),
            ProjectFileLayer::Chip
        );
        assert_eq!(
            default_layer("reviewed-knowledge[0]", ProjectFileOwnership::Reviewed),
            ProjectFileLayer::Investigation
        );
        assert_eq!(
            default_layer("register-facts", ProjectFileOwnership::Generated),
            ProjectFileLayer::Generated
        );
    }

    #[test]
    fn bootstrap_actions_stop_at_the_first_executable_frontier() {
        let mut report = report(vec![
            entry(
                "run-spec",
                ProjectFileOwnership::Local,
                ProjectFileState::Missing,
                Some("project inputs init"),
                true,
            ),
            entry(
                "code-pack",
                ProjectFileOwnership::Reviewed,
                ProjectFileState::Missing,
                None,
                true,
            ),
            entry(
                "symbol-inventory",
                ProjectFileOwnership::Generated,
                ProjectFileState::Pending,
                Some("project analyze"),
                false,
            ),
            entry(
                "published-svd",
                ProjectFileOwnership::Generated,
                ProjectFileState::Pending,
                Some("project publish"),
                false,
            ),
        ]);

        assert_eq!(report.workflow_state(), ProjectFilesWorkflowState::Blocked);
        assert_eq!(
            next_commands(&report),
            [vec!["project", "inputs", "init", "--help"]]
        );

        let run_spec_index = report
            .files
            .iter()
            .position(|file| file.role == "run-spec")
            .unwrap();
        report.files[run_spec_index].state = ProjectFileState::Present;
        assert_eq!(
            next_commands(&report),
            [vec!["advanced", "symbols", "inventory"]]
        );

        let symbol_index = report
            .files
            .iter()
            .position(|file| file.role == "symbol-inventory")
            .unwrap();
        report.files[symbol_index].state = ProjectFileState::Present;
        assert_eq!(
            next_commands(&report),
            [vec!["advanced", "code", "init-pack"]]
        );
    }

    #[test]
    fn publication_is_never_recommended_without_review_configuration() {
        let mut report = report(vec![entry(
            "published-svd",
            ProjectFileOwnership::Generated,
            ProjectFileState::Pending,
            Some("project publish"),
            false,
        )]);

        assert_eq!(
            report.workflow_state(),
            ProjectFilesWorkflowState::ReviewConfigurationRequired
        );
        let steps = next_steps(&report);
        assert_eq!(steps.len(), 1);
        assert_eq!(
            steps[0].instruction,
            "Configure [review] and publication-scopes in vendor-project.toml."
        );
        assert!(steps[0].commands.is_empty());
        assert!(
            next_steps(&report)
                .iter()
                .all(|step| step.commands.is_empty())
        );

        report.files.push(entry(
            "review-workspace",
            ProjectFileOwnership::Generated,
            ProjectFileState::Present,
            Some("project analyze"),
            false,
        ));
        assert_eq!(
            report.workflow_state(),
            ProjectFilesWorkflowState::PublicationPreflightRequired
        );
        assert_eq!(
            next_commands(&report),
            [vec!["project", "publish", "--check"]]
        );
    }

    #[test]
    fn pending_review_outputs_are_not_mislabeled_as_analysis_work() {
        let review = report(vec![
            entry(
                "function-review",
                ProjectFileOwnership::Generated,
                ProjectFileState::Pending,
                Some("advanced functions review"),
                false,
            ),
            entry(
                "register-review",
                ProjectFileOwnership::Generated,
                ProjectFileState::Pending,
                Some("registers review"),
                false,
            ),
            entry(
                "code-review",
                ProjectFileOwnership::Generated,
                ProjectFileState::Pending,
                Some("advanced code review"),
                false,
            ),
            entry(
                "published-svd",
                ProjectFileOwnership::Generated,
                ProjectFileState::Pending,
                Some("project publish"),
                false,
            ),
        ]);

        assert_eq!(
            review.workflow_state(),
            ProjectFilesWorkflowState::ReviewOutputsPending
        );
        assert_eq!(review.pending_analysis_outputs(), 0);
        assert_eq!(review.pending_review_outputs(), 3);
        assert_eq!(
            next_commands(&review),
            [
                vec!["advanced", "code", "review"],
                vec!["registers", "review"],
                vec!["advanced", "functions", "review"],
            ]
        );
    }

    #[test]
    fn completed_analysis_and_complete_file_maps_have_distinct_states() {
        let analysis = report(vec![entry(
            "navigation-index",
            ProjectFileOwnership::Generated,
            ProjectFileState::Pending,
            Some("project analyze"),
            false,
        )]);
        assert_eq!(
            analysis.workflow_state(),
            ProjectFilesWorkflowState::AnalysisPending
        );
        assert_eq!(next_commands(&analysis), [vec!["project", "analyze"]]);

        let complete = report(vec![entry(
            "navigation-index",
            ProjectFileOwnership::Generated,
            ProjectFileState::Present,
            Some("project analyze"),
            false,
        )]);
        assert_eq!(
            complete.workflow_state(),
            ProjectFilesWorkflowState::FilesPresent
        );
        assert_eq!(next_commands(&complete), [vec!["project", "status"]]);
    }
}
