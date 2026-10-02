//! The repository file set every check reads.
//!
//! Paths are repository-relative, `/`-separated and normalized. Only
//! existing files count: a tracked file deleted in the worktree is gone.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

use crate::Result;

/// The checked repository: its root and its files.
pub struct Repo {
    root: PathBuf,
    files: BTreeSet<String>,
    directories: BTreeSet<String>,
}

/// Build output and private inputs never belong to the checked tree.
fn excluded(path: &str) -> bool {
    path.split('/')
        .any(|component| component == "target" || component == "_oracles" || component == ".git")
}

impl Repo {
    /// The tracked and untracked-but-not-ignored files of the Git checkout
    /// at `root`.
    pub fn from_git(root: &Path) -> Result<Self> {
        let output = Command::new("git")
            .arg("-C")
            .arg(root)
            .args([
                "ls-files",
                "-z",
                "--cached",
                "--others",
                "--exclude-standard",
            ])
            .output()
            .map_err(|error| format!("cannot run git ls-files: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "git ls-files failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let listing = String::from_utf8(output.stdout)
            .map_err(|_| "git ls-files printed a non-UTF-8 path".to_owned())?;
        let files = listing
            .split('\0')
            .filter(|path| !path.is_empty() && !excluded(path))
            .filter(|path| root.join(path).is_file())
            .map(str::to_owned);
        Ok(Self::new(root, files))
    }

    /// Every file below `root`, for trees that are not Git checkouts.
    pub fn from_dir(root: &Path) -> Result<Self> {
        let mut files = vec![];
        let mut pending = vec![PathBuf::new()];
        while let Some(relative) = pending.pop() {
            let directory = root.join(&relative);
            let entries = fs::read_dir(&directory)
                .map_err(|error| format!("cannot list {}: {error}", directory.display()))?;
            for entry in entries {
                let entry = entry.map_err(|error| error.to_string())?;
                let name = entry.file_name();
                let Some(name) = name.to_str() else { continue };
                let path = relative.join(name);
                let display = path.to_string_lossy().replace('\\', "/");
                if excluded(&display) {
                    continue;
                }
                let kind = entry.file_type().map_err(|error| error.to_string())?;
                if kind.is_dir() {
                    pending.push(path);
                } else if kind.is_file() {
                    files.push(display);
                }
            }
        }
        Ok(Self::new(root, files))
    }

    fn new(root: &Path, files: impl IntoIterator<Item = String>) -> Self {
        let files: BTreeSet<String> = files.into_iter().collect();
        let mut directories = BTreeSet::new();
        for file in &files {
            let mut path = file.as_str();
            while let Some((parent, _)) = path.rsplit_once('/') {
                if !directories.insert(parent.to_owned()) {
                    break;
                }
                path = parent;
            }
        }
        Self {
            root: root.to_owned(),
            files,
            directories,
        }
    }

    /// The repository root on disk.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Every file, ascending.
    pub fn files(&self) -> impl Iterator<Item = &str> {
        self.files.iter().map(String::as_str)
    }

    /// Whether `path` is a file of the repository.
    pub fn is_file(&self, path: &str) -> bool {
        self.files.contains(path)
    }

    /// Whether `path` names a file or a directory holding files.
    pub fn exists(&self, path: &str) -> bool {
        let path = path.trim_end_matches('/');
        path.is_empty() || self.files.contains(path) || self.directories.contains(path)
    }

    /// The files directly in `directory` (`""` for the root).
    pub fn children<'a>(&'a self, directory: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let prefix = if directory.is_empty() {
            String::new()
        } else {
            format!("{directory}/")
        };
        let length = prefix.len();
        self.files
            .range(prefix.clone()..)
            .take_while(move |file| file.starts_with(&prefix))
            .map(String::as_str)
            .filter(move |file| !file[length..].contains('/'))
    }

    /// The files at any depth below `directory`.
    pub fn below<'a>(&'a self, directory: &'a str) -> impl Iterator<Item = &'a str> + 'a {
        let prefix = format!("{}/", directory.trim_end_matches('/'));
        self.files
            .range(prefix.clone()..)
            .take_while(move |file| file.starts_with(&prefix))
            .map(String::as_str)
    }

    /// The contents of the repository file `path`.
    pub fn read(&self, path: &str) -> Result<String> {
        fs::read_to_string(self.root.join(path))
            .map_err(|error| format!("cannot read {path}: {error}"))
    }
}

/// The directory part of `path` (`""` at the root).
pub fn parent(path: &str) -> &str {
    path.rsplit_once('/').map_or("", |(parent, _)| parent)
}

/// `relative` resolved against the repository directory `base`, normalized;
/// `None` when it is absolute or leaves the repository.
pub fn join(base: &str, relative: &str) -> Option<String> {
    if relative.starts_with('/') {
        return None;
    }
    let mut parts: Vec<&str> = base.split('/').filter(|part| !part.is_empty()).collect();
    for part in relative.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop()?;
            }
            part => parts.push(part),
        }
    }
    Some(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_resolve_parents_and_refuse_to_leave_the_root() {
        assert_eq!(join("a/b", "../c/./d.rs").as_deref(), Some("a/c/d.rs"));
        assert_eq!(join("", "x.rs").as_deref(), Some("x.rs"));
        assert_eq!(join("a", "../../x"), None);
        assert_eq!(join("a", "/etc/passwd"), None);
    }

    #[test]
    fn directories_exist_only_when_they_hold_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("a/b")).unwrap();
        fs::create_dir_all(dir.path().join("empty")).unwrap();
        fs::create_dir_all(dir.path().join("target/debug")).unwrap();
        fs::write(dir.path().join("a/b/c.rs"), "").unwrap();
        fs::write(dir.path().join("target/debug/x.rs"), "").unwrap();
        let repo = Repo::from_dir(dir.path()).unwrap();
        assert!(repo.exists("a") && repo.exists("a/b") && repo.exists("a/b/c.rs"));
        assert!(!repo.exists("empty"));
        assert!(!repo.exists("target/debug/x.rs"));
        assert_eq!(repo.children("a/b").collect::<Vec<_>>(), ["a/b/c.rs"]);
        assert_eq!(repo.children("a").count(), 0);
    }
}
