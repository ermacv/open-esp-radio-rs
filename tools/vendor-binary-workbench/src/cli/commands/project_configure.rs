//! Declarative attachment of a reusable platform pack to a project manifest.

use std::{
    env, fs,
    path::{Component, Path, PathBuf},
};

use toml_edit::{DocumentMut, Item, value};

use super::Result;
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

pub(super) fn run(arguments: Vec<String>, manifest: &Path) -> Result<bool> {
    let options = parse_options(arguments)?;
    if options.selection.is_none() && !options.check {
        return Err(
            "project configure requires --platform-pack PATH, --no-platform-pack, or --check"
                .into(),
        );
    }

    let input = fs::read_to_string(manifest)?;
    let mut document = input.parse::<DocumentMut>()?;
    if document.get("schema").and_then(Item::as_integer) != Some(1) {
        return Err("project manifest requires schema = 1".into());
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
        return Err(format!(
            "project configure staging path exists: {}",
            temporary.display()
        )
        .into());
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
            return Err(format!(
                "project platform configuration differs from {}; rerun without --check",
                manifest.display()
            )
            .into());
        }
    } else if changed {
        fs::rename(&temporary, manifest).inspect_err(|_| {
            let _ = fs::remove_file(&temporary);
        })?;
    }

    let (project, target) = validate_project(manifest)?;
    report(
        if options.check {
            "verified"
        } else if changed {
            "written"
        } else {
            "unchanged"
        },
        &project,
        &target,
        manifest,
    );
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

fn report(status: &str, project: &ProjectSpec, target: &TargetSpec, manifest: &Path) {
    if let Some(pack) = &project.platform_pack {
        println!(
            "PROJECT-CONFIGURE\tstatus={status}\tplatform-pack={}\tharness={}\tsemantic-catalogs={}\tsemantic-operations={}\tmanifest={}",
            pack.id,
            target.harness.as_deref().unwrap_or("-"),
            pack.semantic_catalogs.len(),
            pack.semantic_operations,
            manifest.display(),
        );
    } else {
        println!(
            "PROJECT-CONFIGURE\tstatus={status}\tplatform-pack=-\tharness={}\tsemantic-catalogs=0\tsemantic-operations=0\tmanifest={}",
            target.harness.as_deref().unwrap_or("-"),
            manifest.display(),
        );
    }
}

fn parse_options(arguments: Vec<String>) -> Result<Options> {
    let mut options = Options::default();
    let mut arguments = arguments.into_iter();
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--platform-pack" => {
                let path =
                    PathBuf::from(arguments.next().ok_or("--platform-pack requires a path")?);
                set_selection(&mut options.selection, Selection::Set(path))?;
            }
            "--no-platform-pack" => {
                set_selection(&mut options.selection, Selection::Clear)?;
            }
            "--check" if !options.check => options.check = true,
            "--check" => return Err("duplicate --check".into()),
            _ => return Err(format!("unknown project configure option: {argument}").into()),
        }
    }
    Ok(options)
}

fn set_selection(slot: &mut Option<Selection>, selection: Selection) -> Result<()> {
    if slot.replace(selection).is_some() {
        return Err("project configure accepts exactly one platform-pack selection".into());
    }
    Ok(())
}

fn project_relative_pack_path(manifest: &Path, input: &Path) -> Result<String> {
    let input = if input.is_absolute() {
        input.to_owned()
    } else {
        env::current_dir()?.join(input)
    };
    let input = input
        .canonicalize()
        .map_err(|error| format!("cannot resolve platform pack {}: {error}", input.display()))?;
    let _ = PlatformPack::load(&input)?;
    let base = manifest
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .canonicalize()?;
    relative_path(&base, &input)?
        .to_str()
        .map(str::to_owned)
        .ok_or_else(|| "platform pack path cannot be represented as UTF-8".into())
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
        return Err("project and platform pack do not share a filesystem root".into());
    }
    let mut output = PathBuf::new();
    for _ in &base[common..] {
        output.push(Component::ParentDir.as_os_str());
    }
    for component in &target[common..] {
        output.push(component.as_os_str());
    }
    if output.as_os_str().is_empty() {
        return Err("platform pack path resolves to the project directory".into());
    }
    Ok(output)
}

fn temporary_manifest_path(manifest: &Path) -> Result<PathBuf> {
    let name = manifest
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or("project manifest must have a UTF-8 file name")?;
    Ok(manifest.with_file_name(format!(".{name}.configure-{}", std::process::id())))
}

#[cfg(test)]
mod tests;
