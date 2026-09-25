//! Git pins and root patch replacements resolve identically in every island.
//!
//! Each Cargo workspace island carries its own lock catalog and `[patch]`
//! tables, because Cargo does not inherit them across workspaces. The root
//! workspace owns the canonical patches; every island must apply them, and one
//! Git package resolves to one commit across all islands.

use crate::Result;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

/// A Git repository identity independent of spelling differences Cargo accepts.
fn repository(url: &str) -> String {
    let url = url.trim_end_matches('/');
    url.strip_suffix(".git").unwrap_or(url).to_ascii_lowercase()
}

/// `(repository, commit)` of a lock `source` resolved from Git.
fn git_source(source: &str) -> Option<(String, &str)> {
    let locator = source.strip_prefix("git+")?;
    let (location, commit) = locator.rsplit_once('#')?;
    let url = location.split_once('?').map_or(location, |(url, _)| url);
    Some((repository(url), commit))
}

/// Sources the root manifest replaces, as `(repository, package)` pairs.
fn replaced_sources(root_manifest: &str) -> Result<BTreeSet<(String, String)>> {
    let document: toml::Table = toml::from_str(root_manifest)?;
    let mut replaced = BTreeSet::new();
    let Some(patches) = document.get("patch") else {
        return Ok(replaced);
    };
    let patches = patches.as_table().ok_or("root [patch] is not a table")?;
    for (source, entries) in patches {
        if source == "crates-io" {
            continue;
        }
        let entries = entries
            .as_table()
            .ok_or_else(|| format!("root [patch.{source:?}] is not a table"))?;
        for (name, entry) in entries {
            let package = entry
                .get("package")
                .and_then(toml::Value::as_str)
                .unwrap_or(name);
            replaced.insert((repository(source), package.to_owned()));
        }
    }
    Ok(replaced)
}

/// Rejects islands that skip a root patch or resolve a Git package to a
/// different commit than another island.
pub(super) fn check(root_manifest: &str, locks: &[(PathBuf, String)]) -> Result<()> {
    let replaced = replaced_sources(root_manifest)?;
    let mut commits: BTreeMap<(String, String), BTreeMap<String, BTreeSet<&Path>>> =
        BTreeMap::new();
    let mut errors = Vec::new();
    for (path, contents) in locks {
        let lock: toml::Table =
            toml::from_str(contents).map_err(|error| format!("{}: {error}", path.display()))?;
        let packages = lock
            .get("package")
            .and_then(toml::Value::as_array)
            .map(Vec::as_slice)
            .unwrap_or_default();
        for package in packages {
            let (Some(name), Some(source)) = (
                package.get("name").and_then(toml::Value::as_str),
                package.get("source").and_then(toml::Value::as_str),
            ) else {
                continue;
            };
            let Some((url, commit)) = git_source(source) else {
                continue;
            };
            let key = (url, name.to_owned());
            if replaced.contains(&key) {
                errors.push(format!(
                    "{}: {name} resolves from {}, which the root workspace replaces through [patch]",
                    path.display(),
                    key.0
                ));
            }
            commits
                .entry(key)
                .or_default()
                .entry(commit.to_owned())
                .or_default()
                .insert(path);
        }
    }
    for ((url, name), resolved) in &commits {
        if resolved.len() > 1 {
            let detail = resolved
                .iter()
                .map(|(commit, paths)| {
                    let paths = paths
                        .iter()
                        .map(|path| path.display().to_string())
                        .collect::<Vec<_>>()
                        .join(", ");
                    format!("{commit} in {paths}")
                })
                .collect::<Vec<_>>()
                .join("; ");
            errors.push(format!(
                "{name} from {url} resolves to several commits: {detail}"
            ));
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n").into())
    }
}

#[cfg(test)]
#[path = "pins/tests.rs"]
mod tests;
