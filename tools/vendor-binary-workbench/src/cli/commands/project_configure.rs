//! Declarative attachment of a reusable platform pack to a project manifest.

use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

use toml_edit::{DocumentMut, Item, value};

use serde::Serialize;

use super::Result;
use crate::cli::ProjectConfigureArgs;
use crate::{TargetSpec, platform_pack::PlatformPack, project::ProjectSpec};

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
    platform_pack: Option<String>,
    harness: Option<String>,
    semantic_catalogs: usize,
    semantic_operations: usize,
    manifest: String,
}

pub(super) fn run(arguments: ProjectConfigureArgs, manifest: &Path) -> Result<bool> {
    let options = resolve_options(arguments);
    if options.selection.is_none() && !options.check {
        return Err(crate::Error::invalid(
            "project configure requires --platform-pack PATH, --no-platform-pack, or --check",
        ));
    }

    let input = fs::read_to_string(manifest)?;
    let mut document = input.parse::<DocumentMut>()?;
    if document.get("schema").and_then(Item::as_integer) != Some(1) {
        return Err(crate::Error::invalid(
            "project manifest requires schema = 1",
        ));
    }
    match options.selection {
        Some(Selection::Set(input_path)) => {
            let stored = project_relative_pack_path(manifest, &input_path)?;
            document["platform-pack"] = value(stored);
        }
        Some(Selection::Clear) => {
            document.remove("platform-pack");
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
        schema_version: 1,
        command: "project configure",
        status,
        platform_pack: project.platform_pack.as_ref().map(|pack| pack.id.clone()),
        harness: target.harness.clone(),
        semantic_catalogs: project
            .platform_pack
            .as_ref()
            .map_or(0, |pack| pack.semantic_catalogs.len()),
        semantic_operations: project
            .platform_pack
            .as_ref()
            .map_or(0, |pack| pack.semantic_operations),
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
    if let Some(pack) = &project.platform_pack {
        pack.apply_to_target(&mut target)?;
        if pack.harness.is_some() {
            target.require_available_harness()?;
        }
    }
    Ok((project, target))
}

fn print_report(report: &ProjectConfigureReport) {
    if let Some(pack) = &report.platform_pack {
        outputln!(
            "PROJECT-CONFIGURE\tstatus={status}\tplatform-pack={}\tharness={}\tsemantic-catalogs={}\tsemantic-operations={}\tmanifest={}",
            pack,
            report.harness.as_deref().unwrap_or("-"),
            report.semantic_catalogs,
            report.semantic_operations,
            report.manifest,
            status = report.status,
        );
    } else {
        outputln!(
            "PROJECT-CONFIGURE\tstatus={status}\tplatform-pack=-\tharness={}\tsemantic-catalogs=0\tsemantic-operations=0\tmanifest={}",
            report.harness.as_deref().unwrap_or("-"),
            report.manifest,
            status = report.status,
        );
    }
}

fn resolve_options(arguments: ProjectConfigureArgs) -> Options {
    let selection = arguments
        .platform_pack
        .map(Selection::Set)
        .or_else(|| arguments.no_platform_pack.then_some(Selection::Clear));
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
        .map_err(|error| format!("cannot resolve platform pack {}: {error}", input.display()))
        .map_err(crate::Error::invalid)?;
    let _ = PlatformPack::load(&input)?;
    let base = manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    relative_path(&base, &input)?
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| crate::Error::invalid("platform pack path cannot be represented as UTF-8"))
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
            "project and platform pack do not share a filesystem root",
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
            "platform pack path resolves to the project directory",
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
