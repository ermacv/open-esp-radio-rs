//! Pre-push checks for the files a checkout changed against its base.
//!
//! Several sessions push to `main` directly, so the full CI run is the
//! checkpoint only after the push. This check runs the subset that catches the
//! usual breakage first: formatting of every workspace a changed file belongs
//! to, Clippy and API documentation of the root workspace, the tests of the
//! changed root packages, the Markdown/catalog check when prose changed, and
//! the metadata check when a manifest or lockfile changed. It is not full
//! repository coverage; the CI jobs remain the source checkpoint.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use crate::{Context, Result, cargo, doc, process};

/// What one set of changed files requires.
#[derive(Debug, Default, Eq, PartialEq)]
pub struct Plan {
    /// Workspace manifests whose formatting must be checked.
    pub format: BTreeSet<PathBuf>,
    /// Changed root-workspace packages: tested and documented.
    pub packages: BTreeSet<String>,
    /// A root-workspace source changed: run Clippy over the whole workspace,
    /// because dependents of a changed package can break too.
    pub clippy: bool,
    /// Markdown, qualification catalogs or programs changed.
    pub docs: bool,
    /// A Cargo manifest or lockfile changed.
    pub metadata: bool,
    /// Changed workspaces other than the root, which need their own target
    /// and feature profile to build.
    pub other_workspaces: BTreeSet<PathBuf>,
}

/// One package of the root workspace: its name and directory.
#[derive(Clone, Debug)]
pub struct Member {
    pub name: String,
    pub directory: PathBuf,
}

/// Map changed paths (relative to the root) to the checks they require.
/// `workspace_of` returns the workspace manifest owning a changed Rust or
/// Cargo file, or `None` when no Cargo package owns it.
pub fn plan(
    root: &Path,
    changed: &[PathBuf],
    members: &[Member],
    workspace_of: &mut dyn FnMut(&Path) -> Result<Option<PathBuf>>,
) -> Result<Plan> {
    let root_manifest = root.join("Cargo.toml");
    let mut plan = Plan::default();
    for path in changed {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("");
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        if extension == "md" || path.starts_with("qualification") {
            plan.docs = true;
        }
        if name == "Cargo.toml" || name == "Cargo.lock" {
            plan.metadata = true;
        }
        if extension != "rs" && name != "Cargo.toml" && name != "Cargo.lock" {
            continue;
        }
        let Some(workspace) = workspace_of(&root.join(path))? else {
            continue;
        };
        plan.format.insert(workspace.clone());
        if workspace != root_manifest {
            plan.other_workspaces.insert(workspace);
            continue;
        }
        plan.clippy = true;
        let absolute = root.join(path);
        if let Some(member) = members
            .iter()
            .filter(|member| absolute.starts_with(&member.directory))
            .max_by_key(|member| member.directory.components().count())
        {
            plan.packages.insert(member.name.clone());
        }
    }
    Ok(plan)
}

fn git(ctx: &Context, arguments: &[&str]) -> Result<Vec<PathBuf>> {
    let output = process::capture(ctx.command("git").args(arguments))?;
    Ok(String::from_utf8(output.stdout)?
        .lines()
        .filter(|line| !line.is_empty())
        .map(PathBuf::from)
        .collect())
}

/// Committed, staged, unstaged and untracked changes against the merge base
/// of `HEAD` and `base`. Deleted files are kept: their workspace still needs
/// its checks.
fn changed_files(ctx: &Context, base: &str) -> Result<Vec<PathBuf>> {
    let merge_base = String::from_utf8(
        process::capture(ctx.command("git").args(["merge-base", "HEAD", base]))?.stdout,
    )?;
    let merge_base = merge_base.trim();
    let mut files = BTreeSet::new();
    files.extend(git(ctx, &["diff", "--name-only", merge_base])?);
    files.extend(git(ctx, &["ls-files", "--others", "--exclude-standard"])?);
    Ok(files.into_iter().collect())
}

/// The workspace manifest owning `path`: the nearest `Cargo.toml` above it,
/// resolved by Cargo. Paths under an ignored build directory or without a
/// package have none.
fn workspace_of(
    ctx: &Context,
    cache: &mut BTreeMap<PathBuf, PathBuf>,
    path: &Path,
) -> Result<Option<PathBuf>> {
    let mut directory = path.parent();
    while let Some(current) = directory {
        if !current.starts_with(&ctx.root) {
            return Ok(None);
        }
        let manifest = current.join("Cargo.toml");
        if manifest.is_file() {
            if let Some(workspace) = cache.get(&manifest) {
                return Ok(Some(workspace.clone()));
            }
            let workspace = cargo::workspace_manifest(ctx, &manifest)?;
            cache.insert(manifest, workspace.clone());
            return Ok(Some(workspace));
        }
        directory = current.parent();
    }
    Ok(None)
}

pub fn run(ctx: &Context, base: &str) -> Result<()> {
    let changed = changed_files(ctx, base)?;
    let root_manifest = ctx.root.join("Cargo.toml");
    let metadata = cargo::metadata_no_deps(ctx, &root_manifest)?;
    let members: Vec<Member> = metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
        .filter_map(|package| {
            let directory = package
                .manifest_path
                .parent()?
                .as_std_path()
                .canonicalize()
                .ok()?;
            Some(Member {
                name: package.name.to_string(),
                directory,
            })
        })
        .collect();
    let mut cache = BTreeMap::new();
    let plan = plan(&ctx.root, &changed, &members, &mut |path| {
        workspace_of(ctx, &mut cache, path)
    })?;
    println!(
        "check changed: {} files against {base}; packages: {}",
        changed.len(),
        if plan.packages.is_empty() {
            "none".to_owned()
        } else {
            plan.packages.iter().cloned().collect::<Vec<_>>().join(", ")
        }
    );
    for manifest in &plan.format {
        process::run(
            ctx.cargo()
                .args(["fmt", "--all", "--manifest-path"])
                .arg(manifest)
                .args(["--", "--check"]),
        )?;
    }
    if plan.metadata {
        super::metadata::run(ctx).map(|_| ())?;
    }
    if plan.clippy {
        process::run(ctx.cargo().args([
            "clippy",
            "--workspace",
            "--all-targets",
            "--locked",
            "--offline",
            "--",
            "-D",
            "warnings",
        ]))?;
    }
    if !plan.packages.is_empty() {
        let mut test = ctx.cargo();
        test.args(["test", "--locked", "--offline", "--no-fail-fast"]);
        for package in &plan.packages {
            test.args(["-p", package]);
        }
        process::run(&mut test)?;
        for mut group in doc::groups(&root_manifest, &metadata)? {
            group
                .packages
                .retain(|package| plan.packages.contains(package));
            group.features.retain(|feature| {
                feature
                    .split_once('/')
                    .is_some_and(|(package, _)| plan.packages.contains(package))
            });
            if !group.packages.is_empty() {
                process::run(doc::command(ctx, "doc", &group).arg("--no-deps"))?;
            }
        }
    }
    if plan.docs {
        super::docs::run(ctx)?;
    }
    for workspace in &plan.other_workspaces {
        println!(
            "check changed: formatted {}; build it with its own target and feature profile",
            workspace.display()
        );
    }
    println!("check changed passed; the CI jobs remain the full checkpoint");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn members() -> Vec<Member> {
        vec![
            Member {
                name: "hal".into(),
                directory: "/r/crates/hal".into(),
            },
            Member {
                name: "hal-nested".into(),
                directory: "/r/crates/hal/nested".into(),
            },
        ]
    }

    fn run(changed: &[&str]) -> Plan {
        let changed: Vec<PathBuf> = changed.iter().map(PathBuf::from).collect();
        plan(Path::new("/r"), &changed, &members(), &mut |path| {
            Ok(if path.starts_with("/r/hil/targets") {
                Some("/r/hil/targets/Cargo.toml".into())
            } else if path.starts_with("/r/crates") {
                Some("/r/Cargo.toml".into())
            } else {
                None
            })
        })
        .unwrap()
    }

    #[test]
    fn a_root_source_selects_its_innermost_package_and_workspace_clippy() {
        let plan = run(&["crates/hal/nested/src/lib.rs"]);
        assert_eq!(plan.packages, BTreeSet::from(["hal-nested".to_owned()]));
        assert!(plan.clippy);
        assert_eq!(
            plan.format,
            BTreeSet::from([PathBuf::from("/r/Cargo.toml")])
        );
        assert!(!plan.docs && !plan.metadata);
    }

    #[test]
    fn another_workspace_is_formatted_but_not_built_as_root() {
        let plan = run(&["hil/targets/runtime/src/main.rs"]);
        assert!(plan.packages.is_empty());
        assert!(!plan.clippy);
        assert_eq!(
            plan.other_workspaces,
            BTreeSet::from([PathBuf::from("/r/hil/targets/Cargo.toml")])
        );
    }

    #[test]
    fn prose_and_manifests_select_their_checks_only() {
        let plan = run(&["docs/architecture.md", "crates/hal/Cargo.toml"]);
        assert!(plan.docs);
        assert!(plan.metadata);
        assert_eq!(plan.packages, BTreeSet::from(["hal".to_owned()]));
        let plan = run(&[".github/workflows/ci.yml"]);
        assert_eq!(plan, Plan::default());
    }
}
