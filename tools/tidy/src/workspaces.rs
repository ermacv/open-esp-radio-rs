//! Every package belongs to a workspace as Cargo finds it
//! ([`oer_repo::workspaces`]), every listed member exists, and every
//! workspace root has its lock file and every lock file a root.

use oer_repo::workspaces::{Membership, lock_of, memberships};

use crate::Context;

pub fn check(context: &Context<'_>) -> Vec<String> {
    let manifests = &context.model.manifests;
    let mut problems = vec![];
    for (manifest, membership) in memberships(manifests) {
        if let Membership::Unclaimed(root) = membership {
            let root = if root.is_empty() { "the root" } else { root };
            problems.push(format!(
                "{manifest}: the workspace at {root} neither lists nor excludes this package"
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
    let roots = context.model.workspaces();
    for root in roots {
        let lock = lock_of(root);
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
    use crate::testing::problems;

    const FILES: &[(&str, &str)] = &[
        (
            "Cargo.toml",
            "[workspace]\nmembers = [\"crates/*\", \"firmware/shared\", \"crates/ignored\"]\nexclude = [\"island\", \"firmware\"]\n",
        ),
        ("Cargo.lock", ""),
        (
            "crates/a/Cargo.toml",
            "[package]\nname = \"a\"\n[dependencies]\nb = { path = \"../../shared/b\" }\n",
        ),
        ("shared/b/Cargo.toml", "[package]\nname = \"b\"\n"),
        ("island/Cargo.toml", "[package]\nname = \"island\"\n"),
        (
            "firmware/Cargo.toml",
            "[workspace]\nmembers = [\"app\"]\nexclude = [\"shared\"]\n",
        ),
        ("firmware/Cargo.lock", ""),
        ("firmware/app/Cargo.toml", "[package]\nname = \"app\"\n"),
        (
            "firmware/shared/Cargo.toml",
            "[package]\nname = \"shared\"\n",
        ),
        ("tools/stray/Cargo.toml", "[package]\nname = \"stray\"\n"),
        ("old/Cargo.lock", ""),
    ];

    #[test]
    fn unclaimed_packages_missing_members_and_lock_mismatches_fail() {
        // A member whose manifest the inventory does not see (deleted, or
        // ignored by Git) fails instead of dropping out of every check.
        assert_eq!(
            problems(FILES, check),
            [
                "Cargo.toml: member crates/ignored has no package manifest",
                "island/Cargo.toml: workspace without island/Cargo.lock",
                "old/Cargo.lock: lock file of no workspace root",
                "tools/stray/Cargo.toml: the workspace at the root neither lists nor excludes this package",
            ]
        );
    }
}
