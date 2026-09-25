//! Cargo workspace packages that inventory entries name as implementation owners.
//!
//! Inventory navigation names Cargo packages instead of source files, so a
//! refactor inside a package never edits the catalog. Package directories come
//! from `cargo metadata`, the workspace's own authority on its members.

use super::{BTreeMap, BTreeSet, Path, PathBuf, Result, validate_regular_reference};
use serde::Deserialize;

#[derive(Deserialize)]
struct Metadata {
    workspace_root: PathBuf,
    workspace_members: BTreeSet<String>,
    packages: Vec<Package>,
}

#[derive(Deserialize)]
struct Package {
    id: String,
    name: String,
    manifest_path: PathBuf,
}

/// Repository-relative directory of every workspace member, by package name.
pub(super) struct WorkspacePackages(BTreeMap<String, PathBuf>);

impl WorkspacePackages {
    pub(super) fn load(root: &Path) -> Result<Self> {
        let output =
            std::process::Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
                .current_dir(root)
                .args(["metadata", "--no-deps", "--format-version", "1", "--frozen"])
                .output()?;
        if !output.status.success() {
            return Err(format!(
                "cannot read the Cargo workspace for inventory packages: {}",
                String::from_utf8_lossy(&output.stderr)
            )
            .into());
        }
        Self::from_metadata(&output.stdout)
    }

    fn from_metadata(json: &[u8]) -> Result<Self> {
        let metadata: Metadata = serde_json::from_slice(json)?;
        let mut packages = BTreeMap::new();
        for package in metadata.packages {
            if !metadata.workspace_members.contains(&package.id) {
                continue;
            }
            let directory = package
                .manifest_path
                .parent()
                .and_then(|directory| directory.strip_prefix(&metadata.workspace_root).ok())
                .ok_or_else(|| format!("package {} is outside the workspace", package.name))?
                .to_owned();
            packages.insert(package.name, directory);
        }
        Ok(Self(packages))
    }

    /// Validate one entry's links: known package names and regular non-package documents.
    pub(super) fn validate(
        &self,
        root: &Path,
        entry: &str,
        packages: &[String],
        documents: &[PathBuf],
    ) -> Result<()> {
        let mut names = BTreeSet::new();
        for name in packages {
            if !names.insert(name) {
                return Err(format!("inventory entry {entry} repeats package {name}").into());
            }
            if !self.0.contains_key(name) {
                return Err(format!(
                    "inventory entry {entry} names {name}, which is not a workspace package"
                )
                .into());
            }
        }
        let mut paths = BTreeSet::new();
        for path in documents {
            if !paths.insert(path) {
                return Err(format!(
                    "inventory entry {entry} repeats document {}",
                    path.display()
                )
                .into());
            }
            validate_regular_reference(root, path, "inventory document")?;
            if let Some((name, _)) = self
                .0
                .iter()
                .find(|(_, directory)| path.starts_with(directory))
            {
                return Err(format!(
                    "inventory entry {entry} links {} inside package {name}; name the package instead",
                    path.display()
                )
                .into());
            }
        }
        Ok(())
    }

    pub(super) fn directories<'a>(
        &self,
        packages: impl IntoIterator<Item = &'a String>,
    ) -> BTreeMap<String, PathBuf> {
        packages
            .into_iter()
            .filter_map(|name| Some((name.clone(), self.0.get(name)?.clone())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packages() -> WorkspacePackages {
        WorkspacePackages::from_metadata(
            br#"{"workspace_root":"/repo","workspace_members":["phy-id"],"packages":[
                {"id":"phy-id","name":"phy","manifest_path":"/repo/crates/phy/Cargo.toml"},
                {"id":"outside-id","name":"outside","manifest_path":"/repo/tools/outside/Cargo.toml"}
            ]}"#,
        )
        .unwrap()
    }

    #[test]
    fn only_workspace_members_resolve_to_repository_directories() {
        let packages = packages();
        let names = ["phy".to_owned(), "outside".to_owned()];
        assert_eq!(
            packages.directories(&names),
            BTreeMap::from([("phy".into(), PathBuf::from("crates/phy"))])
        );
    }

    #[test]
    fn links_name_members_once_and_keep_documents_outside_packages() {
        let root = std::env::temp_dir().join(format!("oer-inventory-links-{}", std::process::id()));
        std::fs::create_dir_all(root.join("crates/phy/src")).unwrap();
        std::fs::create_dir_all(root.join("docs")).unwrap();
        std::fs::write(root.join("crates/phy/src/lib.rs"), "").unwrap();
        std::fs::write(root.join("docs/phy.md"), "").unwrap();
        let packages = packages();
        let phy = ["phy".to_owned()];
        let document = [PathBuf::from("docs/phy.md")];
        packages.validate(&root, "entry", &phy, &document).unwrap();
        for (names, documents) in [
            (vec!["phy".to_owned(), "phy".to_owned()], vec![]),
            (vec!["outside".to_owned()], vec![]),
            (vec![], vec![PathBuf::from("crates/phy/src/lib.rs")]),
            (vec![], vec![PathBuf::from("docs/missing.md")]),
        ] {
            assert!(
                packages
                    .validate(&root, "entry", &names, &documents)
                    .is_err()
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }
}
