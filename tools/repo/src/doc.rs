//! Build API documentation the way docs.rs does: each package once, for the
//! target and features in its `[package.metadata.docs.rs]` table.
//!
//! Cargo cannot document packages of one workspace for different targets in a
//! single invocation, so packages are grouped by target and every group runs
//! one ordinary `cargo doc --no-deps`. Doctests are host tests independent of
//! the documentation target: `cargo test --doc --workspace` runs them all.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::{Context, Result, cargo, checks::common, process};

/// One `cargo doc` invocation: a workspace, an optional target and packages.
#[derive(Debug, Eq, PartialEq)]
pub struct Group {
    pub manifest: PathBuf,
    pub target: Option<String>,
    pub packages: Vec<String>,
    /// Package-qualified features, such as `crate/feature`.
    pub features: Vec<String>,
}

/// Group workspace members by their docs.rs target. Only `default-target`,
/// an empty `targets` list and `features` are supported; any other docs.rs
/// setting is rejected so that no configuration is silently ignored.
pub fn groups(manifest: &Path, metadata: &cargo_metadata::Metadata) -> Result<Vec<Group>> {
    let mut groups: BTreeMap<Option<String>, Group> = BTreeMap::new();
    for package in metadata
        .packages
        .iter()
        .filter(|package| metadata.workspace_members.contains(&package.id))
    {
        let docs = package.metadata.get("docs").and_then(|docs| docs.get("rs"));
        let mut target = None;
        let mut features = Vec::new();
        if let Some(docs) = docs {
            let table = docs
                .as_object()
                .ok_or_else(|| format!("{}: docs.rs metadata must be a table", package.name))?;
            for (key, value) in table {
                match key.as_str() {
                    "default-target" => {
                        target = Some(
                            value
                                .as_str()
                                .ok_or_else(|| format!("{}: default-target", package.name))?
                                .to_owned(),
                        );
                    }
                    "targets" if value.as_array().is_some_and(Vec::is_empty) => {}
                    "features" => {
                        for feature in value
                            .as_array()
                            .ok_or_else(|| format!("{}: features", package.name))?
                        {
                            let feature = feature
                                .as_str()
                                .ok_or_else(|| format!("{}: features", package.name))?;
                            features.push(format!("{}/{feature}", package.name));
                        }
                    }
                    other => {
                        return Err(format!(
                            "{}: unsupported docs.rs setting `{other}`",
                            package.name
                        )
                        .into());
                    }
                }
            }
        }
        let group = groups.entry(target.clone()).or_insert_with(|| Group {
            manifest: manifest.to_owned(),
            target,
            packages: Vec::new(),
            features: Vec::new(),
        });
        group.packages.push(package.name.to_string());
        group.features.extend(features);
    }
    Ok(groups.into_values().collect())
}

fn command(ctx: &Context, subcommand: &str, group: &Group) -> std::process::Command {
    let mut command = ctx.cargo();
    let mut flags = std::env::var("RUSTDOCFLAGS").unwrap_or_default();
    flags.push_str(" -D warnings");
    command
        .env("RUSTDOCFLAGS", flags.trim())
        .args([subcommand, "--locked", "--offline", "--manifest-path"])
        .arg(&group.manifest);
    if let Some(target) = &group.target {
        command.args(["--target", target]);
    }
    for package in &group.packages {
        command.args(["-p", package]);
    }
    if !group.features.is_empty() {
        command.args(["--features", &group.features.join(",")]);
    }
    command
}

/// Document the root workspace and every other workspace that owns a
/// production package, then run the root workspace's doctests on the host.
pub fn run(ctx: &Context) -> Result<()> {
    let mut manifests = vec![ctx.root.join("Cargo.toml").canonicalize()?];
    for package in common::production_packages(ctx)? {
        if !package.workspace_member {
            let manifest = cargo::workspace_manifest(ctx, &package.manifest)?;
            if !manifests.contains(&manifest) {
                manifests.push(manifest);
            }
        }
    }
    for manifest in manifests {
        for group in groups(&manifest, &cargo::metadata_no_deps(ctx, &manifest)?)? {
            process::run(command(ctx, "doc", &group).arg("--no-deps"))?;
        }
    }
    process::run(
        ctx.cargo()
            .args(["test", "--doc", "--workspace", "--locked", "--offline"]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata(packages: serde_json::Value) -> cargo_metadata::Metadata {
        let members: Vec<_> = packages
            .as_array()
            .unwrap()
            .iter()
            .map(|p| p["id"].clone())
            .collect();
        serde_json::from_value(serde_json::json!({
            "packages": packages, "workspace_members": members, "resolve": null,
            "target_directory": "/w/target", "version": 1, "workspace_root": "/w"
        }))
        .unwrap()
    }

    fn package(name: &str, docs: serde_json::Value) -> serde_json::Value {
        serde_json::json!({
            "name": name, "version": "0.1.0", "id": format!("path+file:///w/{name}#0.1.0"),
            "license": null, "license_file": null, "description": null, "source": null,
            "dependencies": [], "targets": [], "features": {},
            "manifest_path": format!("/w/{name}/Cargo.toml"),
            "metadata": docs, "publish": null, "authors": [], "categories": [],
            "keywords": [], "readme": null, "repository": null, "homepage": null,
            "documentation": null, "edition": "2024", "links": null, "default_run": null,
            "rust_version": null
        })
    }

    #[test]
    fn packages_are_grouped_by_their_docs_rs_target_and_features() {
        let chip = serde_json::json!({"docs": {"rs": {
            "default-target": "riscv32imafc-unknown-none-elf", "targets": [], "features": ["a"]
        }}});
        let groups = groups(
            Path::new("/w/Cargo.toml"),
            &metadata(serde_json::json!([
                package("host", serde_json::Value::Null),
                package("chip", chip),
            ])),
        )
        .unwrap();
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].target, None);
        assert_eq!(groups[0].packages, ["host"]);
        assert_eq!(
            groups[1].target.as_deref(),
            Some("riscv32imafc-unknown-none-elf")
        );
        assert_eq!(groups[1].features, ["chip/a"]);
    }

    #[test]
    fn unsupported_docs_rs_settings_are_rejected() {
        let docs = serde_json::json!({"docs": {"rs": {"all-features": true}}});
        let error = groups(
            Path::new("/w/Cargo.toml"),
            &metadata(serde_json::json!([package("crate", docs)])),
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("unsupported docs.rs setting `all-features`"));
    }
}
