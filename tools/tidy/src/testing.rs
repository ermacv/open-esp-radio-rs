//! Temporary fixture trees for the checks' tests.

use std::fs;

use tempfile::TempDir;

use crate::{Context, repo::Repo};

/// A temporary directory holding `files` as `(path, contents)`.
pub fn tree(files: &[(&str, &str)]) -> TempDir {
    let dir = tempfile::tempdir().unwrap();
    for (path, contents) in files {
        let path = dir.path().join(path);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }
    dir
}

/// Runs `check` over a fixture tree of `files`.
pub fn problems(
    files: &[(&str, &str)],
    check: impl FnOnce(&Context<'_>) -> Vec<String>,
) -> Vec<String> {
    let dir = tree(files);
    let repo = Repo::from_dir(dir.path()).unwrap();
    let context = Context::load(&repo).unwrap();
    check(&context)
}
