//! Every package belongs to a workspace as Cargo finds it
//! ([`oer_repo::workspaces`]), every listed member exists, and every
//! workspace root has its lock file and every lock file a root; every
//! firmware workspace builds with the same release profile.

use oer_repo::classification::Platform;
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

/// Every firmware workspace (one whose classified members all run on a chip
/// or family and none on the host: examples, platforms, HIL targets,
/// verification probes) declares one and the same `[profile.release]`, so
/// the units they share in the image compile cache are compiled alike.
pub fn release_profiles(context: &Context<'_>) -> Vec<String> {
    let model = &context.model;
    let mut profiles = vec![];
    let mut problems = vec![];
    for root in model.workspaces() {
        let platforms: Vec<_> = model
            .members(root)
            .filter_map(|package| model.classification(package).ok())
            .map(|class| &class.platform)
            .collect();
        let firmware = !platforms.is_empty()
            && platforms
                .iter()
                .all(|platform| matches!(platform, Platform::Chip(_) | Platform::Family(_)));
        if !firmware {
            continue;
        }
        let release = context
            .repo
            .read(root)
            .map_err(|error| error.to_string())
            .and_then(|text| {
                text.parse::<toml::Table>()
                    .map_err(|error| error.to_string())
            })
            .map(|table| {
                table
                    .get("profile")
                    .and_then(|profile| profile.get("release"))
                    .cloned()
            });
        match release {
            Ok(Some(release)) => profiles.push((root.as_str(), release)),
            Ok(None) => problems.push(format!(
                "{root}: firmware workspace without [profile.release]"
            )),
            Err(error) => problems.push(format!("{root}: {error}")),
        }
    }
    if let Some((first, reference)) = profiles.first() {
        for (root, release) in &profiles[1..] {
            if release != reference {
                problems.push(format!(
                    "{root}: [profile.release] differs from {first}'s; every firmware workspace builds with the same release profile"
                ));
            }
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

    const CHIP: (&str, &str) = (
        "platform/chip-a/chip.toml",
        "schema = 1\nid = \"chip-a\"\nfamily = \"espressif\"\nrust-target = \"riscv32imafc-unknown-none-elf\"\nboot = \"staged\"\nespflash-chip = \"chip-a\"\nrevisions = [\"rev0\"]\n[properties]\nwifi-bands = [\"2g4\"]\nbluetooth = [\"le\"]\nieee802154 = true\ncores = 2\n",
    );
    const FIRMWARE: &str = "[package.metadata.open-radio]\nlayer = \"application\"\nplatform = \"chip\"\nchip = \"chip-a\"\n";
    const HOST: &str = "[package.metadata.open-radio]\nlayer = \"tool\"\nplatform = \"host\"\nhost-layer = \"foundation\"\n";

    #[test]
    fn firmware_workspaces_share_one_release_profile() {
        let a = format!("[package]\nname = \"a\"\n{FIRMWARE}");
        let b = format!("[package]\nname = \"b\"\n{FIRMWARE}");
        let c = format!("[package]\nname = \"c\"\n{FIRMWARE}");
        let h = format!("[package]\nname = \"h\"\n{HOST}");
        let files = [
            CHIP,
            (
                "one/Cargo.toml",
                "[workspace]\nmembers = [\"a\"]\n[profile.release]\nopt-level = 3\nlto = \"fat\"\n",
            ),
            ("one/Cargo.lock", ""),
            ("one/a/Cargo.toml", a.as_str()),
            // Key order does not matter; values do.
            (
                "two/Cargo.toml",
                "[workspace]\nmembers = [\"b\"]\n[profile.release]\nlto = \"fat\"\nopt-level = \"s\"\n",
            ),
            ("two/Cargo.lock", ""),
            ("two/b/Cargo.toml", b.as_str()),
            ("three/Cargo.toml", "[workspace]\nmembers = [\"c\"]\n"),
            ("three/Cargo.lock", ""),
            ("three/c/Cargo.toml", c.as_str()),
            // A host workspace keeps its own profile.
            (
                "host/Cargo.toml",
                "[workspace]\nmembers = [\"h\"]\n[profile.release]\nopt-level = 1\n",
            ),
            ("host/Cargo.lock", ""),
            ("host/h/Cargo.toml", h.as_str()),
        ];
        assert_eq!(
            problems(&files, release_profiles),
            [
                "three/Cargo.toml: firmware workspace without [profile.release]",
                "two/Cargo.toml: [profile.release] differs from one/Cargo.toml's; every firmware workspace builds with the same release profile",
            ]
        );
    }
}
