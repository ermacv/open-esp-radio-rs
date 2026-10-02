//! Cargo workspaces discovered from the manifests, and their consistency.
//!
//! A workspace is a manifest with `[workspace]`, or a package that no
//! workspace above it claims or excludes into a root of its own. Tooling
//! that iterates every workspace (CI formatting) takes this list instead of
//! keeping its own.

use std::collections::{BTreeMap, BTreeSet};

use crate::{
    Context,
    manifest::{Manifests, Package},
    repo::parent,
};

/// Where a package belongs.
enum Membership<'a> {
    /// A member of the workspace above it.
    Member,
    /// Its own workspace root, explicit or implicit.
    Root,
    /// A workspace above it neither lists nor excludes it.
    Unclaimed(&'a str),
}

/// Whether the workspace at `workspace` excludes `directory`: below an
/// `exclude` entry and not below an explicit member, as Cargo decides.
fn excludes(manifests: &Manifests, workspace: &str, directory: &str) -> bool {
    let below = |root: &String| directory == root || directory.starts_with(&format!("{root}/"));
    manifests
        .workspaces
        .iter()
        .filter(|declared| declared.directory == workspace)
        .any(|declared| declared.exclude.iter().any(below) && !declared.members.iter().any(below))
}

/// The directory of the nearest workspace declaration above `directory`
/// that does not exclude it, as Cargo searches for a package's root.
fn enclosing<'a>(manifests: &'a Manifests, directory: &str) -> Option<&'a str> {
    let mut current = directory;
    while !current.is_empty() {
        current = parent(current);
        if let Some(workspace) = manifests
            .workspaces
            .iter()
            .find(|workspace| workspace.directory == current)
            && !excludes(manifests, current, directory)
        {
            return Some(&workspace.directory);
        }
    }
    None
}

/// Members of every workspace: listed members and, transitively, their
/// path dependencies inside the workspace directory.
fn members(manifests: &Manifests) -> BTreeMap<&str, BTreeSet<String>> {
    let mut all = BTreeMap::new();
    for workspace in &manifests.workspaces {
        let inside = |path: &str| {
            workspace.directory.is_empty() || path.starts_with(&format!("{}/", workspace.directory))
        };
        let mut set: BTreeSet<String> = workspace.members.iter().cloned().collect();
        let mut pending: Vec<String> = set.iter().cloned().collect();
        while let Some(member) = pending.pop() {
            let Some(package) = manifests.package_at(&member) else {
                continue;
            };
            for dependency in &package.dependencies {
                if let Some(path) = &dependency.path
                    && inside(path)
                    && !workspace
                        .exclude
                        .iter()
                        .any(|excluded| path.starts_with(excluded.as_str()))
                    && set.insert(path.clone())
                {
                    pending.push(path.clone());
                }
            }
        }
        all.insert(workspace.directory.as_str(), set);
    }
    all
}

fn membership<'a>(
    manifests: &'a Manifests,
    members: &BTreeMap<&str, BTreeSet<String>>,
    package: &'a Package,
) -> Membership<'a> {
    if manifests
        .workspaces
        .iter()
        .any(|workspace| workspace.directory == package.directory)
    {
        return Membership::Root;
    }
    let root = match &package.workspace {
        Some(root) => Some(root.as_str()),
        None => enclosing(manifests, &package.directory),
    };
    let Some(root) = root else {
        return Membership::Root;
    };
    if members
        .get(root)
        .is_some_and(|set| set.contains(&package.directory))
    {
        Membership::Member
    } else {
        Membership::Unclaimed(root)
    }
}

/// Manifest paths of every workspace root, ascending.
pub fn discover(manifests: &Manifests) -> Vec<String> {
    let members = members(manifests);
    let mut roots: BTreeSet<String> = manifests
        .workspaces
        .iter()
        .map(|workspace| workspace.manifest.clone())
        .collect();
    for package in &manifests.packages {
        if let Membership::Root = membership(manifests, &members, package) {
            roots.insert(package.manifest.clone());
        }
    }
    roots.into_iter().collect()
}

/// Every package belongs to a workspace, every listed member exists, and
/// every workspace root has its lock file and every lock file a root.
pub fn check(context: &Context<'_>) -> Vec<String> {
    let manifests = &context.manifests;
    let members = members(manifests);
    let mut problems = vec![];
    for package in &manifests.packages {
        if let Membership::Unclaimed(root) = membership(manifests, &members, package) {
            let root = if root.is_empty() { "the root" } else { root };
            problems.push(format!(
                "{}: the workspace at {root} neither lists nor excludes this package",
                package.manifest
            ));
        }
    }
    for workspace in &manifests.workspaces {
        for member in &workspace.members {
            if manifests.package_at(member).is_none() {
                problems.push(format!(
                    "{}: member {member} has no package manifest",
                    workspace.manifest
                ));
            }
        }
    }
    let roots = discover(manifests);
    for root in &roots {
        let lock = format!("{}Cargo.lock", root.trim_end_matches("Cargo.toml"));
        if !context.repo.is_file(&lock) {
            problems.push(format!("{root}: workspace without {lock}"));
        }
    }
    for lock in context
        .repo
        .files()
        .filter(|file| *file == "Cargo.lock" || file.ends_with("/Cargo.lock"))
    {
        let manifest = format!("{}Cargo.toml", lock.trim_end_matches("Cargo.lock"));
        if !roots.contains(&manifest) {
            problems.push(format!("{lock}: lock file of no workspace root"));
        }
    }
    problems.sort();
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        repo::Repo,
        testing::{problems, tree},
    };

    const FILES: &[(&str, &str)] = &[
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\", \"firmware/shared\"]\nexclude = [\"island\", \"firmware\"]\n",
        ),
        ("Cargo.lock", ""),
        (
            "crates/a/Cargo.toml",
            "[package]\nname = \"a\"\n[dependencies]\nb = { path = \"../../shared/b\" }\n",
        ),
        ("shared/b/Cargo.toml", "[package]\nname = \"b\"\n"),
        ("island/Cargo.toml", "[package]\nname = \"island\"\n"),
        ("island/Cargo.lock", ""),
        (
            "firmware/Cargo.toml",
            "[workspace]\nmembers = [\"app\"]\nexclude = [\"shared\"]\n",
        ),
        (
            "firmware/shared/Cargo.toml",
            "[package]\nname = \"shared\"\n",
        ),
        ("firmware/Cargo.lock", ""),
        ("firmware/app/Cargo.toml", "[package]\nname = \"app\"\n"),
    ];

    #[test]
    fn explicit_and_implicit_workspace_roots_are_discovered() {
        let dir = tree(FILES);
        let repo = Repo::from_dir(dir.path()).unwrap();
        let manifests = Manifests::load(&repo).unwrap();
        assert_eq!(
            discover(&manifests),
            ["Cargo.toml", "firmware/Cargo.toml", "island/Cargo.toml"]
        );
        assert!(problems(FILES, check).is_empty());
    }

    #[test]
    fn unclaimed_packages_and_lock_mismatches_fail() {
        let mut files = FILES.to_vec();
        files.retain(|(path, _)| *path != "island/Cargo.lock");
        files.push(("tools/stray/Cargo.toml", "[package]\nname = \"stray\"\n"));
        files.push(("old/Cargo.lock", ""));
        assert_eq!(
            problems(&files, check),
            [
                "island/Cargo.toml: workspace without island/Cargo.lock",
                "old/Cargo.lock: lock file of no workspace root",
                "tools/stray/Cargo.toml: the workspace at the root neither lists nor excludes this package",
            ]
        );
    }
}
