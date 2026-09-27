//! The repository files a firmware image was built from.
//!
//! Cargo writes the source files of every crate linked into a binary to its
//! dep-info file (`<binary>.d`), and each build script's `rerun-if-changed`
//! inputs to its `output`. Together they name the repository files that
//! produced the image, so evidence recorded from it stays valid while those
//! files are unchanged. Files outside the repository belong to locked
//! registry or Git packages and are identified by the lockfiles instead.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Component, Path, PathBuf},
};

use crate::Result;

/// Format of `source-inputs.json`.
pub const SCHEMA: u32 = 1;

/// Repository-relative files the binaries below each `(release directory,
/// binary name)` were built from, plus the manifest and build script of each
/// repository package they compiled.
pub fn collect(repository: &Path, builds: &[(&Path, &str)]) -> Result<BTreeSet<PathBuf>> {
    let repository = repository.canonicalize()?;
    let mut files = BTreeSet::new();
    for (release, binary) in builds {
        let dep_info = fs::read_to_string(release.join(format!("{binary}.d")))?;
        for path in dependencies(&dep_info) {
            if let Some(relative) = inside(&repository, &path) {
                files.insert(relative);
            }
        }
    }
    let packages = packages(&repository, &files)?;
    for directory in packages.values() {
        for file in ["Cargo.toml", "build.rs"] {
            if repository.join(directory).join(file).is_file() {
                files.insert(directory.join(file));
            }
        }
    }
    for (release, _) in builds {
        let Ok(scripts) = fs::read_dir(release.join("build")) else {
            continue;
        };
        for script in scripts.flatten() {
            let Ok(output) = fs::read_to_string(script.path().join("output")) else {
                continue;
            };
            let name = script.file_name().to_string_lossy().into_owned();
            let package = name
                .rsplit_once('-')
                .and_then(|(package, _)| packages.get(package));
            for line in output.lines() {
                let Some(path) = line
                    .strip_prefix("cargo::rerun-if-changed=")
                    .or_else(|| line.strip_prefix("cargo:rerun-if-changed="))
                else {
                    continue;
                };
                let path = Path::new(path);
                let absolute = match (path.is_absolute(), package) {
                    (true, _) => path.to_owned(),
                    (false, Some(directory)) => repository.join(directory).join(path),
                    // A relative input of a registry or Git package.
                    (false, None) => continue,
                };
                if let Some(relative) = inside(&repository, &absolute)
                    && repository.join(&relative).is_file()
                {
                    files.insert(relative);
                }
            }
        }
    }
    Ok(files)
}

/// Write `files` as `source-inputs.json` below `output`.
pub fn write(output: &Path, files: &BTreeSet<PathBuf>) -> Result<PathBuf> {
    let path = output.join("source-inputs.json");
    crate::durable::atomic_json(
        &path,
        &serde_json::json!({"schema": SCHEMA, "files": files}),
    )?;
    Ok(path)
}

/// The paths of a dep-info file, whose spaces inside a path are escaped.
fn dependencies(dep_info: &str) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for line in dep_info.lines() {
        let Some((_, list)) = line.split_once(": ") else {
            continue;
        };
        let mut current = String::new();
        let mut characters = list.chars().peekable();
        while let Some(character) = characters.next() {
            match character {
                '\\' if characters.peek() == Some(&' ') => {
                    current.push(' ');
                    characters.next();
                }
                ' ' => {
                    if !current.is_empty() {
                        paths.push(PathBuf::from(std::mem::take(&mut current)));
                    }
                }
                character => current.push(character),
            }
        }
        if !current.is_empty() {
            paths.push(PathBuf::from(current));
        }
    }
    paths
}

/// `path` relative to `repository` after resolving `.` and `..` lexically,
/// when it lies inside it and outside `target/`.
fn inside(repository: &Path, path: &Path) -> Option<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                normalized.pop();
            }
            Component::CurDir => {}
            component => normalized.push(component),
        }
    }
    let relative = normalized.strip_prefix(repository).ok()?.to_owned();
    (!relative.starts_with("target") && relative.components().next().is_some()).then_some(relative)
}

/// The repository packages owning `files`, by package name.
fn packages(repository: &Path, files: &BTreeSet<PathBuf>) -> Result<BTreeMap<String, PathBuf>> {
    let mut packages = BTreeMap::new();
    for file in files {
        let Some(directory) = file
            .ancestors()
            .skip(1)
            .find(|directory| repository.join(directory).join("Cargo.toml").is_file())
        else {
            continue;
        };
        let manifest: toml::Value = toml::from_str(&fs::read_to_string(
            repository.join(directory).join("Cargo.toml"),
        )?)?;
        // A workspace manifest has no `[package]`.
        if let Some(name) = manifest
            .get("package")
            .and_then(|package| package.get("name"))
            .and_then(|name| name.as_str())
        {
            packages.insert(name.to_owned(), directory.to_owned());
        }
    }
    Ok(packages)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn inputs_are_the_compiled_sources_and_repository_build_script_inputs() {
        let directory = tempfile::tempdir().unwrap();
        let repository = directory.path().join("repo");
        let root = repository.canonicalize().unwrap_or_else(|_| {
            fs::create_dir_all(&repository).unwrap();
            repository.canonicalize().unwrap()
        });
        write_file(
            &root.join("crates/radio/Cargo.toml"),
            "[package]\nname = \"oer-radio\"\n",
        );
        write_file(&root.join("crates/radio/build.rs"), "");
        write_file(&root.join("crates/radio/src/lib.rs"), "");
        write_file(&root.join("crates/radio/src/my file.rs"), "");
        write_file(&root.join("crates/radio/memory.x"), "");
        write_file(&root.join("crates/other/src/lib.rs"), "");
        // A workspace manifest owns a compiled file without being a package.
        write_file(&root.join("Cargo.toml"), "[workspace]\n");
        write_file(&root.join("tool.rs"), "");
        write_file(&root.join("platform/linker/link.x"), "");
        let release = directory.path().join("target/release");
        write_file(
            &release.join("app.d"),
            &format!(
                "{0}/target/release/app: {0}/crates/radio/src/lib.rs {0}/crates/radio/src/my\\ file.rs {0}/tool.rs /registry/esp-hal/src/lib.rs\n",
                root.display()
            ),
        );
        write_file(
            &release.join("build/oer-radio-0123abcd/output"),
            &format!(
                "cargo:rerun-if-changed=memory.x\ncargo::rerun-if-changed={}/crates/radio/../../platform/linker/link.x\ncargo:rustc-link-arg=-Tlink.x\n",
                root.display()
            ),
        );
        write_file(
            &release.join("build/esp-hal-89ab/output"),
            "cargo:rerun-if-changed=ld/memory.x\n",
        );
        let files = collect(&root, &[(&release, "app")]).unwrap();
        let expected = [
            "crates/radio/Cargo.toml",
            "crates/radio/build.rs",
            "crates/radio/memory.x",
            "crates/radio/src/lib.rs",
            "crates/radio/src/my file.rs",
            "platform/linker/link.x",
            "tool.rs",
        ]
        .map(PathBuf::from);
        assert_eq!(files.into_iter().collect::<Vec<_>>(), expected);
    }
}
