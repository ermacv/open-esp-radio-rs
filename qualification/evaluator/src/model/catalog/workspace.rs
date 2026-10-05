//! Repository packages that inventory entries name as implementation owners.
//!
//! Inventory navigation names Cargo packages instead of source files, so a
//! refactor inside a package never edits the catalog. Package directories come
//! from the repository model (`oer-repo`), across every workspace.

use super::{BTreeMap, BTreeSet, Path, PathBuf, Result, validate_regular_reference};

/// Repository-relative directory of every package, by package name.
pub(super) struct WorkspacePackages(BTreeMap<String, PathBuf>);

impl WorkspacePackages {
    pub(super) fn load(root: &Path) -> Result<Self> {
        let model = oer_repo::Model::load(&oer_repo::Repo::load(root)?)?;
        let mut packages = BTreeMap::new();
        for package in model.packages() {
            if packages
                .insert(package.name.clone(), PathBuf::from(&package.directory))
                .is_some()
            {
                return Err(format!(
                    "package name {} is declared twice; inventory entries need unique names",
                    package.name
                )
                .into());
            }
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
                    "inventory entry {entry} names {name}, which is not a repository package"
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

    /// A repository with the package `phy` in the root workspace and
    /// `outside` in a workspace of its own.
    fn repository(tag: &str) -> (PathBuf, WorkspacePackages) {
        let root = std::env::temp_dir().join(format!("oer-inventory-{tag}-{}", std::process::id()));
        for (path, text) in [
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/phy\"]\nexclude = [\"tools\"]\n",
            ),
            ("crates/phy/Cargo.toml", "[package]\nname = \"phy\"\n"),
            ("crates/phy/src/lib.rs", ""),
            (
                "tools/outside/Cargo.toml",
                "[package]\nname = \"outside\"\n[workspace]\n",
            ),
            ("docs/phy.md", ""),
        ] {
            let path = root.join(path);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, text).unwrap();
        }
        let packages = WorkspacePackages::load(&root).unwrap();
        (root, packages)
    }

    #[test]
    fn packages_of_every_workspace_resolve_to_repository_directories() {
        let (root, packages) = repository("directories");
        let names = ["phy".to_owned(), "outside".to_owned(), "absent".to_owned()];
        assert_eq!(
            packages.directories(&names),
            BTreeMap::from([
                ("outside".into(), PathBuf::from("tools/outside")),
                ("phy".into(), PathBuf::from("crates/phy")),
            ])
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn links_name_packages_once_and_keep_documents_outside_packages() {
        let (root, packages) = repository("links");
        let phy = ["phy".to_owned()];
        let document = [PathBuf::from("docs/phy.md")];
        packages.validate(&root, "entry", &phy, &document).unwrap();
        for (names, documents) in [
            (vec!["phy".to_owned(), "phy".to_owned()], vec![]),
            (vec!["absent".to_owned()], vec![]),
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
