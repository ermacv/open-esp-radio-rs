//! Every workspace island applies the root lint policy.
//!
//! Cargo does not inherit `[workspace.lints]` across workspaces. Each island
//! repeats the root table and each package opts in with `lints.workspace`, so
//! crates built outside the root workspace keep the same deny list.

use crate::Result;
use std::path::{Path, PathBuf};

/// Blobray is extracted and built standalone and owns its own lint policy.
const INDEPENDENT_POLICY: &str = "tools/blobray/Cargo.toml";

/// Generated register bindings cannot satisfy `unsafe_op_in_unsafe_fn`.
const OWN_POLICY_PACKAGES: &[&str] = &["oer-esp32s31-pac-raw"];

/// One Cargo workspace and the manifests of its members.
pub(super) struct Island {
    /// Workspace manifest, relative to the repository root.
    pub(super) manifest: PathBuf,
    pub(super) contents: String,
    /// `(package name, manifest path, manifest contents)` of each member.
    pub(super) members: Vec<(String, PathBuf, String)>,
}

fn workspace_lints(contents: &str) -> Result<Option<toml::Value>> {
    let document: toml::Table = toml::from_str(contents)?;
    Ok(document
        .get("workspace")
        .and_then(|workspace| workspace.get("lints"))
        .cloned())
}

fn opts_in(contents: &str) -> Result<bool> {
    let document: toml::Table = toml::from_str(contents)?;
    let expected: toml::Value = toml::Value::Table(toml::toml! { workspace = true });
    Ok(document.get("lints") == Some(&expected))
}

/// Rejects islands whose policy differs from the root and packages that do
/// not inherit their workspace policy.
pub(super) fn check(root: &Path, islands: &[Island]) -> Result<()> {
    let root_island = islands
        .iter()
        .find(|island| island.manifest == root)
        .ok_or("root workspace manifest missing from the islands")?;
    let policy =
        workspace_lints(&root_island.contents)?.ok_or("root workspace declares no lint policy")?;
    let mut errors = Vec::new();
    for island in islands {
        if island.manifest == Path::new(INDEPENDENT_POLICY) {
            continue;
        }
        if workspace_lints(&island.contents)?.as_ref() != Some(&policy) {
            errors.push(format!(
                "{}: [workspace.lints] differs from the root workspace policy",
                island.manifest.display()
            ));
        }
        for (name, manifest, contents) in &island.members {
            if !OWN_POLICY_PACKAGES.contains(&name.as_str()) && !opts_in(contents)? {
                errors.push(format!(
                    "{}: package {name} must declare `[lints] workspace = true`",
                    manifest.display()
                ));
            }
        }
    }
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors.join("\n").into())
    }
}

#[cfg(test)]
#[path = "lints/tests.rs"]
mod tests;
