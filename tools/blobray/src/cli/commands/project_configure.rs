//! Declarative attachment of reusable ecosystem semantics to a project.

use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

use toml_edit::{Array, DocumentMut, Item, value};

use serde::Serialize;

use super::Result;
use crate::cli::ProjectConfigureArgs;
use crate::{TargetSpec, ecosystem_pack::EcosystemPack, project::ProjectSpec};

#[derive(Debug, Eq, PartialEq)]
enum Selection {
    Set(PathBuf),
    Clear,
}

#[derive(Debug, Default, Eq, PartialEq)]
struct Options {
    selection: Option<Selection>,
    check: bool,
}

#[derive(Serialize)]
struct ProjectConfigureReport {
    schema_version: u32,
    command: &'static str,
    status: &'static str,
    ecosystem_packs: Vec<String>,
    knowledge_provider: Option<String>,
    knowledge_packs: usize,
    knowledge_operations: usize,
    manifest: String,
}

pub(super) fn run(arguments: ProjectConfigureArgs, manifest: &Path) -> Result<bool> {
    let options = resolve_options(arguments);
    if options.selection.is_none() && !options.check {
        return Err(crate::Error::invalid(
            "project configure requires --ecosystem-pack PATH, --no-ecosystem-pack, or --check",
        ));
    }

    let input = fs::read_to_string(manifest)?;
    let mut document = input.parse::<DocumentMut>()?;
    if document.get("schema").and_then(Item::as_integer) != Some(3) {
        return Err(crate::Error::invalid(
            "project manifest requires schema = 3",
        ));
    }
    match options.selection {
        Some(Selection::Set(input_path)) => {
            let stored = project_relative_pack_path(manifest, &input_path)?;
            let mut packs = Array::new();
            packs.push(stored);
            document["ecosystem-packs"] = value(packs);
        }
        Some(Selection::Clear) => {
            document.remove("ecosystem-packs");
        }
        None => {}
    }
    let rendered = document.to_string();
    let changed = rendered != input;
    let temporary = temporary_manifest_path(manifest)?;
    if temporary.exists() {
        return Err(crate::Error::invalid(format!(
            "project configure staging path exists: {}",
            temporary.display()
        )));
    }

    if changed {
        fs::write(&temporary, &rendered)?;
    }
    let candidate = if changed {
        temporary.as_path()
    } else {
        manifest
    };
    let validation = validate_project(candidate);
    if let Err(error) = validation {
        if changed {
            let _ = fs::remove_file(&temporary);
        }
        return Err(error);
    }

    if options.check {
        if changed {
            let _ = fs::remove_file(&temporary);
            return Err(crate::Error::invalid(format!(
                "project platform configuration differs from {}; rerun without --check",
                manifest.display()
            )));
        }
    } else if changed {
        fs::rename(&temporary, manifest).inspect_err(|_| {
            let _ = fs::remove_file(&temporary);
        })?;
    }

    let (project, target) = validate_project(manifest)?;
    let status = if options.check {
        "verified"
    } else if changed {
        "written"
    } else {
        "unchanged"
    };
    let report = ProjectConfigureReport {
        schema_version: 2,
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
    };
    if !crate::cli::output::structured(&report) {
        print_report(&report);
    }
    Ok(true)
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

fn print_report(report: &ProjectConfigureReport) {
    outputln!("{}", crate::cli::output::heading("Project configuration"));
    outputln!(
        "{}",
        crate::cli::output::success(format!("{} — configuration is valid", report.status))
    );
    outputln!("\nManifest: {}", report.manifest);
    if !report.ecosystem_packs.is_empty() {
        outputln!("Ecosystem packs:     {}", report.ecosystem_packs.join(", "));
        outputln!(
            "Knowledge provider:  {}",
            report.knowledge_provider.as_deref().unwrap_or("none")
        );
        outputln!("Knowledge packs:     {}", report.knowledge_packs);
        outputln!("Knowledge operations:{}", report.knowledge_operations);
    } else {
        outputln!("Ecosystem packs: none");
        outputln!(
            "Knowledge provider: {}",
            report.knowledge_provider.as_deref().unwrap_or("none")
        );
    }
    outputln!("\n{}", crate::cli::output::heading("Next"));
    outputln!("1. blobray project doctor --project {}", report.manifest);
}

fn resolve_options(arguments: ProjectConfigureArgs) -> Options {
    let selection = arguments
        .ecosystem_pack
        .map(Selection::Set)
        .or_else(|| arguments.no_ecosystem_pack.then_some(Selection::Clear));
    Options {
        selection,
        check: arguments.check,
    }
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
