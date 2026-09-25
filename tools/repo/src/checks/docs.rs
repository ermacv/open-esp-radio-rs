//! Static documentation checks: local Markdown links and qualification catalogs.
//!
//! API documentation is built by `cargo xtask doc`, and the guides in `docs/`
//! by `mdbook build docs`; this check covers what neither of them validates.

use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
};

use super::common;
use crate::{Context, Result, paths, process};

mod markdown;
use markdown::check_markdown;

/// Check every owned Markdown document's local links, and check and render
/// the static qualification catalogs and programs.
pub fn run(ctx: &Context) -> Result<()> {
    let packages = common::source_packages(ctx)?;
    let mut documents = owned_documents(ctx, &packages)?;
    let groups = catalog_groups(ctx)?;
    let output = ctx.root.join("target/docs");
    fs::create_dir_all(&output)?;
    documents.extend(run_catalogs(ctx, &output, &groups)?);
    let links = check_markdown(ctx, &documents)?;
    println!(
        "docs static passed: catalogs={} programs={} documents={} local-links={} anchors={} external-not-checked={}",
        groups
            .iter()
            .map(|group| group.catalogs.len())
            .sum::<usize>(),
        groups
            .iter()
            .map(|group| group.programs.len())
            .sum::<usize>(),
        links.documents,
        links.local_links,
        links.anchors,
        links.external_not_checked,
    );
    Ok(())
}

#[derive(Clone, Debug)]
struct CatalogGroup {
    chip: String,
    catalogs: Vec<PathBuf>,
    programs: Vec<PathBuf>,
}

#[derive(Debug)]
struct LinkSummary {
    documents: usize,
    local_links: usize,
    external_not_checked: usize,
    anchors: usize,
}

fn catalog_groups(ctx: &Context) -> Result<Vec<CatalogGroup>> {
    let mut groups = BTreeMap::<String, CatalogGroup>::new();
    for path in paths::source_files(ctx)? {
        let relative = path.strip_prefix(&ctx.root)?;
        let parts = relative.components().collect::<Vec<_>>();
        let classify = |owner: &str| {
            parts.len() == 4
                && parts[0] == Component::Normal("qualification".as_ref())
                && parts[1] == Component::Normal(owner.as_ref())
                && path
                    .extension()
                    .is_some_and(|extension| extension == "toml")
        };
        let destination = if classify("catalog") {
            Some(true)
        } else if classify("targets") {
            Some(false)
        } else {
            None
        };
        let Some(is_catalog) = destination else {
            continue;
        };
        let chip = parts[2]
            .as_os_str()
            .to_str()
            .ok_or("qualification chip directory is not Unicode")?
            .to_owned();
        let group = groups.entry(chip.clone()).or_insert_with(|| CatalogGroup {
            chip,
            catalogs: Vec::new(),
            programs: Vec::new(),
        });
        if is_catalog {
            group.catalogs.push(relative.to_owned());
        } else {
            group.programs.push(relative.to_owned());
        }
    }
    for group in groups.values_mut() {
        group.catalogs.sort();
        group.programs.sort();
        if group.catalogs.is_empty() {
            return Err(format!(
                "qualification target group {} has no source catalog",
                group.chip
            )
            .into());
        }
    }
    if groups.is_empty() {
        return Err("no qualification catalog groups found".into());
    }
    Ok(groups.into_values().collect())
}

fn qualification_binary(ctx: &Context) -> Result<PathBuf> {
    // All scopes use the same catalog tool; avoid recompiling it per scope.
    let target = ctx.root.join("target");
    process::run(ctx.cargo().env("CARGO_TARGET_DIR", &target).args([
        "build",
        "--locked",
        "--offline",
        "--profile",
        "qualification",
        "--package",
        "open-esp-radio-qualification-check",
        "--bin",
        "open-esp-radio-qualification-check",
    ]))?;
    let binary = target.join("qualification").join(format!(
        "open-esp-radio-qualification-check{}",
        std::env::consts::EXE_SUFFIX
    ));
    if !binary.is_file() {
        return Err(format!("qualification binary missing: {}", binary.display()).into());
    }
    Ok(binary)
}

fn qualification_command(
    ctx: &Context,
    binary: &Path,
    arguments: impl IntoIterator<Item = OsString>,
) -> Result<String> {
    let mut command = ctx.command(binary);
    command.args(arguments).args(["--root"]).arg(&ctx.root);
    let output = process::capture(&mut command)?;
    let stdout = String::from_utf8(output.stdout)?;
    print!("{stdout}");
    Ok(stdout)
}

fn run_catalogs(ctx: &Context, output: &Path, groups: &[CatalogGroup]) -> Result<Vec<PathBuf>> {
    let binary = qualification_binary(ctx)?;
    run_catalog_actions(output, groups, |group, action| {
        let mut arguments = vec![OsString::from("catalog")];
        match action {
            CatalogAction::CheckCatalogs => {
                arguments.push("check".into());
                for catalog in &group.catalogs {
                    arguments.push("--catalog".into());
                    arguments.push(catalog.as_os_str().to_owned());
                }
            }
            CatalogAction::CheckProgram(program) => {
                arguments.extend(["check".into(), "--manifest".into()]);
                arguments.push(program.as_os_str().to_owned());
            }
            CatalogAction::Render {
                catalogs,
                destination,
            } => {
                arguments.push("render".into());
                for catalog in catalogs {
                    arguments.push("--catalog".into());
                    arguments.push(catalog.as_os_str().to_owned());
                }
                arguments.push("--out".into());
                arguments.push(destination.as_os_str().to_owned());
            }
        }
        qualification_command(ctx, &binary, arguments).map(|_| ())
    })?;

    let mut generated = Vec::new();
    for group in groups {
        let base = output.join("catalogs").join(&group.chip);
        let forward = base.join("forward");
        let reverse = base.join("reverse");
        for name in [
            "project-status.md",
            "domain-inventory.md",
            "capability-catalog.md",
            "migration-map.md",
        ] {
            let left = forward.join(name);
            let right = reverse.join(name);
            if fs::read(&left)? != fs::read(&right)? {
                return Err(format!(
                    "catalog render is input-order dependent for {}",
                    left.display()
                )
                .into());
            }
            generated.push(left);
        }
    }
    Ok(generated)
}

enum CatalogAction<'a> {
    CheckCatalogs,
    CheckProgram(&'a Path),
    Render {
        catalogs: &'a [PathBuf],
        destination: PathBuf,
    },
}

fn run_catalog_actions(
    output: &Path,
    groups: &[CatalogGroup],
    mut execute: impl FnMut(&CatalogGroup, CatalogAction<'_>) -> Result<()>,
) -> Result<()> {
    for group in groups {
        execute(group, CatalogAction::CheckCatalogs)?;
        for program in &group.programs {
            execute(group, CatalogAction::CheckProgram(program))?;
        }
        let base = output.join("catalogs").join(&group.chip);
        execute(
            group,
            CatalogAction::Render {
                catalogs: &group.catalogs,
                destination: base.join("forward"),
            },
        )?;
        let mut reversed = group.catalogs.clone();
        reversed.reverse();
        execute(
            group,
            CatalogAction::Render {
                catalogs: &reversed,
                destination: base.join("reverse"),
            },
        )?;
    }
    Ok(())
}

fn owned_documents(ctx: &Context, packages: &[common::SourcePackage]) -> Result<Vec<PathBuf>> {
    let tracked = paths::tracked_files(ctx)?
        .into_iter()
        .collect::<BTreeSet<_>>();
    let mut documents = BTreeSet::new();
    for path in paths::source_files(ctx)? {
        if path.extension().is_none_or(|extension| extension != "md") {
            continue;
        }
        let relative = path.strip_prefix(&ctx.root)?;
        let owner_name = path.file_name().is_some_and(|name| {
            name == "README.md" || name == "FEATURES.md" || name == "OWNERSHIP.md"
        });
        if tracked.contains(&path)
            || relative.starts_with("docs")
            || relative == Path::new("CONTRIBUTING.md")
            || owner_name
        {
            documents.insert(path);
        }
    }
    for item in packages {
        if let Some(readme) = &item.package.readme {
            let path = item
                .manifest
                .parent()
                .ok_or("package manifest has no parent")?
                .join(readme.as_std_path());
            if !path.is_file() {
                return Err(format!(
                    "package {} readme is missing: {}",
                    item.package.name,
                    path.display()
                )
                .into());
            }
            documents.insert(path.canonicalize()?);
        }
    }
    if documents.is_empty() {
        return Err("no owned Markdown documents found".into());
    }
    Ok(documents.into_iter().collect())
}

#[cfg(test)]
mod tests;
