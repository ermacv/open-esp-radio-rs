//! Typed inventory of project inputs, reviewed knowledge and generated outputs.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

use serde::Serialize;

use super::ProjectContext;
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
    pub(crate) state: ProjectFileState,
    pub(crate) path: PathBuf,
    pub(crate) producer: Option<String>,
    pub(crate) consumers: Vec<String>,
    pub(crate) required: bool,
    pub(crate) next_action: Option<String>,
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

pub(crate) fn collect(context: ProjectContext<'_>) -> Result<ProjectFilesReport> {
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

    let run_spec_path = configured_run_spec(context.project_path, project.run_spec.as_deref());
    push(
        &mut files,
        "run-spec",
        ProjectFileOwnership::Local,
        &run_spec_path,
        Some("project inputs init"),
        &["artifact bindings", "verification"],
        true,
        Some("blobray project inputs init"),
    );
    if run_spec_path.is_file() {
        for input in RunSpec::load(&run_spec_path)?.inputs() {
            let next_action = format!(
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
                Some(&next_action),
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

    files.sort_by(|left, right| {
        (left.ownership, left.role.as_str(), left.path.as_os_str()).cmp(&(
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
        schema: 1,
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
    let next_action = format!("blobray {producer}");
    push(
        files,
        role,
        ProjectFileOwnership::Generated,
        path,
        Some(producer),
        consumers,
        false,
        Some(&next_action),
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
    next_action: Option<&str>,
) {
    let state = if path.exists() {
        ProjectFileState::Present
    } else if ownership == ProjectFileOwnership::Generated {
        ProjectFileState::Pending
    } else {
        ProjectFileState::Missing
    };
    files.push(ProjectFileEntry {
        role: role.into(),
        ownership,
        state,
        path: path.to_owned(),
        producer: producer.map(str::to_owned),
        consumers: consumers.iter().map(|value| (*value).to_owned()).collect(),
        required,
        next_action: (state != ProjectFileState::Present)
            .then(|| next_action.map(str::to_owned))
            .flatten(),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
