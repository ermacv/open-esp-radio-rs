//! Frontend-independent validation and publication of project composition.

use std::{
    env, fs,
    io::Write as _,
    path::{Component, Path, PathBuf},
};

use serde::Serialize;
use toml_edit::{Array, DocumentMut, Item, value};

use super::{ApplicationResult, ExecutableAction, FollowUpStep, ProjectContextRequirement};
use crate::{Result, TargetSpec, ecosystem_pack::EcosystemPack, project::ProjectSpec};

/// A replacement composition. `None` inspects the existing selection;
/// `Some([])` removes it, and a nonempty list preserves the supplied order.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ProjectConfigureRequest {
    pub ecosystem_packs: Option<Vec<PathBuf>>,
    pub check: bool,
}

#[derive(Debug, Serialize)]
pub struct ProjectConfigureReport {
    pub schema_version: u32,
    pub command: &'static str,
    pub status: &'static str,
    pub ecosystem_packs: Vec<String>,
    pub knowledge_provider: Option<String>,
    pub knowledge_packs: usize,
    pub knowledge_operations: usize,
    pub manifest: String,
    pub next_steps: Vec<FollowUpStep>,
}

/// Validate the complete candidate before atomically replacing the manifest.
pub fn configure_project(
    manifest: &Path,
    request: ProjectConfigureRequest,
) -> ApplicationResult<ProjectConfigureReport> {
    let manifest = manifest
        .canonicalize()
        .map_err(|error| crate::Error::read("project manifest", manifest, error))?;
    Ok(configure(&manifest, request)?)
}

fn configure(manifest: &Path, request: ProjectConfigureRequest) -> Result<ProjectConfigureReport> {
    if request.ecosystem_packs.is_none() && !request.check {
        return Err(crate::Error::invalid(
            "project configuration requires an ecosystem selection or check mode",
        ));
    }
    let next_steps = vec![FollowUpStep::command(
        "Validate the resolved project, local inputs, and reviewed workspaces.",
        ExecutableAction::new(
            vec![
                "blobray".to_owned(),
                "project".to_owned(),
                "doctor".to_owned(),
                "--project".to_owned(),
                executable_path(manifest, "project manifest")?,
            ],
            env::current_dir()?,
            ProjectContextRequirement::ProjectOnly,
        )?,
    )];

    let input = fs::read_to_string(manifest)?;
    let mut document = input.parse::<DocumentMut>()?;
    if document.get("schema").and_then(Item::as_integer) != Some(4) {
        return Err(crate::Error::invalid(
            "project manifest requires schema = 4",
        ));
    }
    if let Some(input_paths) = request.ecosystem_packs {
        if input_paths.is_empty() {
            document.remove("ecosystem-packs");
        } else {
            let mut packs = Array::new();
            for input_path in input_paths {
                let stored = project_relative_pack_path(manifest, &input_path)?;
                if packs.iter().any(|item| item.as_str() == Some(&stored)) {
                    return Err(crate::Error::invalid(format!(
                        "ecosystem pack is selected more than once: {stored}"
                    )));
                }
                packs.push(stored);
            }
            document["ecosystem-packs"] = value(packs);
        }
    }
    let rendered = document.to_string();
    let changed = rendered != input;
    let mut staged = changed
        .then(|| StagedManifest::create(manifest, &rendered))
        .transpose()?;
    let candidate = staged
        .as_ref()
        .map_or(manifest, |staged| staged.path.as_path());
    let (project, target) = validate_project(candidate)?;

    if request.check {
        if changed {
            return Err(crate::Error::invalid(format!(
                "project platform configuration differs from {}; rerun without --check",
                manifest.display()
            )));
        }
    } else if let Some(staged) = &mut staged {
        staged.publish(manifest)?;
    }

    let status = if request.check {
        "verified"
    } else if changed {
        "written"
    } else {
        "unchanged"
    };
    let report = ProjectConfigureReport {
        schema_version: 3,
        command: "project configure",
        status,
        ecosystem_packs: project
            .ecosystem_packs
            .iter()
            .map(|pack| pack.id.clone())
            .collect(),
        knowledge_provider: target.knowledge_provider.clone(),
        knowledge_packs: project
            .ecosystem_packs
            .iter()
            .map(|pack| pack.knowledge_packs.len())
            .sum::<usize>()
            + project
                .chip_pack
                .as_ref()
                .map_or(0, |pack| pack.knowledge_packs.len()),
        knowledge_operations: project
            .ecosystem_packs
            .iter()
            .map(|pack| pack.knowledge_operations)
            .sum::<usize>()
            + project
                .chip_pack
                .as_ref()
                .map_or(0, |pack| pack.knowledge_operations),
        manifest: manifest.display().to_string(),
        next_steps,
    };
    Ok(report)
}

/// Own only the staging file created by this call. Failure during writing or
/// validation leaves the original manifest intact and removes our candidate.
struct StagedManifest {
    path: PathBuf,
    published: bool,
}

impl StagedManifest {
    fn create(manifest: &Path, rendered: &str) -> Result<Self> {
        let path = temporary_manifest_path(manifest)?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let staged = Self {
            path,
            published: false,
        };
        let result = file
            .write_all(rendered.as_bytes())
            .and_then(|()| file.sync_all());
        drop(file);
        result?;
        Ok(staged)
    }

    fn publish(&mut self, manifest: &Path) -> Result<()> {
        fs::rename(&self.path, manifest)?;
        self.published = true;
        Ok(())
    }
}

impl Drop for StagedManifest {
    fn drop(&mut self) {
        if !self.published {
            let _ = fs::remove_file(&self.path);
        }
    }
}

fn validate_project(manifest: &Path) -> Result<(ProjectSpec, TargetSpec)> {
    let project = ProjectSpec::load(manifest)?;
    let mut target = TargetSpec::load(&project.target_spec)?;
    project.apply_to_target(&mut target)?;
    if target.knowledge_provider.is_some() {
        target.require_available_knowledge_provider()?;
    }
    Ok((project, target))
}

fn executable_path(path: &Path, role: &str) -> Result<String> {
    path.to_str().map(str::to_owned).ok_or_else(|| {
        crate::Error::invalid(format!(
            "{role} path is not valid UTF-8 and cannot be represented in an executable action: {}",
            path.display()
        ))
    })
}

fn project_relative_pack_path(manifest: &Path, input: &Path) -> Result<String> {
    let input = if input.is_absolute() {
        input.to_owned()
    } else {
        env::current_dir()?.join(input)
    };
    let input = input
        .canonicalize()
        .map_err(|error| format!("cannot resolve ecosystem pack {}: {error}", input.display()))
        .map_err(crate::Error::invalid)?;
    let _ = EcosystemPack::load(&input)?;
    let base = manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    relative_path(&base, &input)?
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| crate::Error::invalid("ecosystem pack path cannot be represented as UTF-8"))
}

fn relative_path(base: &Path, target: &Path) -> Result<PathBuf> {
    let base = base.components().collect::<Vec<_>>();
    let target = target.components().collect::<Vec<_>>();
    let common = base
        .iter()
        .zip(&target)
        .take_while(|(left, right)| left == right)
        .count();
    if common == 0 {
        return Err(crate::Error::invalid(
            "project and ecosystem pack do not share a filesystem root",
        ));
    }
    let mut output = PathBuf::new();
    for _ in &base[common..] {
        output.push(Component::ParentDir.as_os_str());
    }
    for component in &target[common..] {
        output.push(component.as_os_str());
    }
    if output.as_os_str().is_empty() {
        return Err(crate::Error::invalid(
            "ecosystem pack path resolves to the project directory",
        ));
    }
    Ok(output)
}

fn temporary_manifest_path(manifest: &Path) -> Result<PathBuf> {
    let name = manifest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("project manifest must have a UTF-8 file name")
        .map_err(crate::Error::invalid)?;
    Ok(manifest.with_file_name(format!(".{name}.configure-{}", std::process::id())))
}

#[cfg(test)]
mod tests;
