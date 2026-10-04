//! Cargo manifests read without Cargo: packages, their target roots and
//! dependencies, and workspace declarations.

use toml::{Table, Value};

use crate::{
    Result,
    repo::{Repo, join, parent},
};

/// One dependency declaration of a package.
#[derive(Clone, Debug)]
pub struct Dependency {
    /// The key: the name the package's code uses, before `-` becomes `_`.
    pub key: String,
    /// The table it sits in, such as `dev-dependencies` or
    /// `target.'cfg(unix)'.dependencies`.
    pub table: String,
    /// The repository path of a path dependency's directory.
    pub path: Option<String>,
    /// The features it enables in its own spec.
    pub features: Vec<String>,
}

/// One package manifest.
#[derive(Clone, Debug)]
pub struct Package {
    /// Repository path of its `Cargo.toml`.
    pub manifest: String,
    /// Repository path of its directory.
    pub directory: String,
    pub name: String,
    /// The library crate name, when it has a library.
    pub library: String,
    /// The explicit `package.workspace` root directory, if any.
    pub workspace: Option<String>,
    /// Crate roots of every target, including the build script.
    pub roots: Vec<String>,
    /// Explicit target paths that name no file.
    pub missing_roots: Vec<String>,
    pub dependencies: Vec<Dependency>,
    /// Dependency keys that `[features]` forward features to (`key/feature`).
    pub feature_forwarded: Vec<String>,
    /// Every entry of every `[features]` list, such as `esp-hal?/esp32s31`.
    pub feature_entries: Vec<String>,
    /// The `[package.metadata.open-radio]` table, when declared.
    pub open_radio: Option<Table>,
}

/// One `[workspace]` declaration.
#[derive(Clone, Debug)]
pub struct Workspace {
    pub manifest: String,
    pub directory: String,
    /// Member directories, globs expanded.
    pub members: Vec<String>,
    /// Excluded directories.
    pub exclude: Vec<String>,
}

/// Every manifest of the repository.
pub struct Manifests {
    pub packages: Vec<Package>,
    pub workspaces: Vec<Workspace>,
}

fn is_manifest(path: &str) -> bool {
    path == "Cargo.toml" || path.ends_with("/Cargo.toml")
}

impl Manifests {
    pub fn load(repo: &Repo) -> Result<Self> {
        let mut packages = vec![];
        let mut workspaces = vec![];
        for manifest in repo.files().filter(|path| is_manifest(path)) {
            let text = repo.read(manifest)?;
            let table: Table = text
                .parse()
                .map_err(|error| format!("{manifest}: invalid TOML: {error}"))?;
            let directory = parent(manifest).to_owned();
            if let Some(workspace) = table.get("workspace").and_then(Value::as_table) {
                workspaces.push(Workspace {
                    manifest: manifest.to_owned(),
                    members: expand_members(repo, &directory, strings(workspace.get("members"))),
                    exclude: strings(workspace.get("exclude"))
                        .iter()
                        .filter_map(|path| join(&directory, path))
                        .collect(),
                    directory: directory.clone(),
                });
            }
            if table.contains_key("package") {
                packages.push(package(repo, manifest, &directory, &table)?);
            }
        }
        Ok(Self {
            packages,
            workspaces,
        })
    }

    /// The package whose directory is `directory`.
    pub fn package_at(&self, directory: &str) -> Option<&Package> {
        self.packages.iter().find(|p| p.directory == directory)
    }
}

pub(crate) fn strings(value: Option<&Value>) -> Vec<String> {
    match value {
        Some(Value::String(s)) => vec![s.clone()],
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => vec![],
    }
}

/// Member patterns with a trailing `*` component expanded against the
/// directories holding a `Cargo.toml`.
fn expand_members(repo: &Repo, directory: &str, patterns: Vec<String>) -> Vec<String> {
    let mut members = vec![];
    for pattern in patterns {
        let Some(path) = join(directory, &pattern) else {
            continue;
        };
        if path.contains('*') {
            let (prefix, rest) = path.split_once('*').unwrap_or((&path, ""));
            for manifest in repo.files().filter(|path| is_manifest(path)) {
                let candidate = parent(manifest);
                if let Some(tail) = candidate.strip_prefix(prefix)
                    && !tail.is_empty()
                    && glob_tail(tail, rest)
                {
                    members.push(candidate.to_owned());
                }
            }
        } else {
            members.push(path);
        }
    }
    members
}

/// Whether `tail` (after the first `*`) matches the remainder of a member
/// pattern: one path component, then `rest` verbatim.
fn glob_tail(tail: &str, rest: &str) -> bool {
    match tail.find('/') {
        None => rest.is_empty(),
        Some(slash) => &tail[slash..] == rest,
    }
}

fn auto(package: &Table, key: &str) -> bool {
    package.get(key).and_then(Value::as_bool) != Some(false)
}

fn package(repo: &Repo, manifest: &str, directory: &str, table: &Table) -> Result<Package> {
    let section = table
        .get("package")
        .and_then(Value::as_table)
        .ok_or_else(|| format!("{manifest}: [package] is not a table"))?;
    let name = section
        .get("name")
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{manifest}: package without a name"))?
        .to_owned();
    let at = |path: &str| join(directory, path);
    let mut roots = vec![];
    let mut missing_roots = vec![];
    let mut unnamed = vec![];
    let mut add = |path: Option<String>, explicit: bool| {
        if let Some(path) = path {
            if repo.is_file(&path) {
                roots.push(path);
            } else if explicit {
                missing_roots.push(path);
            }
        }
    };

    let lib = table.get("lib").and_then(Value::as_table);
    let library = lib
        .and_then(|lib| lib.get("name"))
        .and_then(Value::as_str)
        .unwrap_or(&name)
        .replace('-', "_");
    match lib.and_then(|lib| lib.get("path")).and_then(Value::as_str) {
        Some(path) => add(at(path), true),
        None => add(at("src/lib.rs"), false),
    }
    match section.get("build") {
        Some(Value::String(path)) => add(at(path), true),
        Some(Value::Boolean(false)) => {}
        _ => add(at("build.rs"), false),
    }
    if auto(section, "autobins") {
        add(at("src/main.rs"), false);
    }
    for (key, directory_name, auto_key) in [
        ("bin", "src/bin", "autobins"),
        ("test", "tests", "autotests"),
        ("bench", "benches", "autobenches"),
        ("example", "examples", "autoexamples"),
    ] {
        for target in table
            .get(key)
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_table)
        {
            if let Some(path) = target.get("path").and_then(Value::as_str) {
                add(at(path), true);
                continue;
            }
            let Some(target_name) = target.get("name").and_then(Value::as_str) else {
                continue;
            };
            let mut candidates = vec![
                format!("{directory_name}/{target_name}.rs"),
                format!("{directory_name}/{target_name}/main.rs"),
            ];
            if key == "bin" {
                candidates.push("src/main.rs".to_owned());
            }
            let found = candidates
                .iter()
                .filter_map(|candidate| at(candidate))
                .find(|path| repo.is_file(path));
            match found {
                Some(path) => add(Some(path), false),
                None => unnamed.push(format!("{key} target `{target_name}`")),
            }
        }
        if auto(section, auto_key)
            && let Some(base) = at(directory_name)
        {
            let found: Vec<String> = repo
                .children(&base)
                .filter(|file| file.ends_with(".rs"))
                .map(str::to_owned)
                .chain(
                    repo.below(&base)
                        .filter(|file| {
                            file.strip_prefix(&format!("{base}/"))
                                .is_some_and(|rest| rest.matches('/').count() == 1)
                                && file.ends_with("/main.rs")
                        })
                        .map(str::to_owned),
                )
                .collect();
            for path in found {
                add(Some(path), false);
            }
        }
    }
    roots.sort();
    roots.dedup();
    missing_roots.extend(unnamed);

    let mut dependencies = vec![];
    let mut collect = |prefix: &str, table: &Table| {
        for kind in ["dependencies", "dev-dependencies", "build-dependencies"] {
            let Some(deps) = table.get(kind).and_then(Value::as_table) else {
                continue;
            };
            for (key, value) in deps {
                let path = value
                    .as_table()
                    .and_then(|spec| spec.get("path"))
                    .and_then(Value::as_str)
                    .and_then(|path| join(directory, path));
                let features = strings(value.as_table().and_then(|spec| spec.get("features")));
                dependencies.push(Dependency {
                    key: key.clone(),
                    table: format!("{prefix}{kind}"),
                    path,
                    features,
                });
            }
        }
    };
    collect("", table);
    if let Some(targets) = table.get("target").and_then(Value::as_table) {
        for (cfg, target) in targets {
            if let Some(target) = target.as_table() {
                collect(&format!("target.'{cfg}'."), target);
            }
        }
    }

    let mut feature_forwarded = vec![];
    let mut feature_entries = vec![];
    if let Some(features) = table.get("features").and_then(Value::as_table) {
        for enabled in features.values().flat_map(|v| strings(Some(v))) {
            if let Some((key, _)) = enabled.split_once('/') {
                feature_forwarded.push(key.trim_end_matches('?').to_owned());
            }
            feature_entries.push(enabled);
        }
    }

    Ok(Package {
        manifest: manifest.to_owned(),
        directory: directory.to_owned(),
        name,
        library,
        workspace: section
            .get("workspace")
            .and_then(Value::as_str)
            .and_then(|path| join(directory, path)),
        roots,
        missing_roots,
        dependencies,
        feature_forwarded,
        feature_entries,
        open_radio: section
            .get("metadata")
            .and_then(|metadata| metadata.get("open-radio"))
            .and_then(Value::as_table)
            .cloned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::tree;

    #[test]
    fn targets_follow_cargo_discovery_and_explicit_paths() {
        let dir = tree(&[
            (
                "p/Cargo.toml",
                "[package]\nname = \"p-x\"\n[[bin]]\nname = \"tool\"\npath = \"tool.rs\"\n[[test]]\nname = \"gone\"\npath = \"tests/gone.rs\"\n",
            ),
            ("p/src/lib.rs", ""),
            ("p/tool.rs", ""),
            ("p/build.rs", ""),
            ("p/tests/a.rs", ""),
            ("p/tests/b/main.rs", ""),
            ("p/tests/b/helper.rs", ""),
            ("p/examples/e.rs", ""),
        ]);
        let repo = Repo::from_dir(dir.path()).unwrap();
        let manifests = Manifests::load(&repo).unwrap();
        let package = &manifests.packages[0];
        assert_eq!(package.library, "p_x");
        assert_eq!(
            package.roots,
            [
                "p/build.rs",
                "p/examples/e.rs",
                "p/src/lib.rs",
                "p/tests/a.rs",
                "p/tests/b/main.rs",
                "p/tool.rs"
            ]
        );
        assert_eq!(package.missing_roots, ["p/tests/gone.rs"]);
    }

    #[test]
    fn workspace_member_globs_expand_to_package_directories() {
        let dir = tree(&[
            (
                "Cargo.toml",
                "[workspace]\nmembers = [\"crates/*\", \"tool\"]\n",
            ),
            ("crates/a/Cargo.toml", "[package]\nname = \"a\"\n"),
            ("crates/b/c/Cargo.toml", "[package]\nname = \"c\"\n"),
            ("tool/Cargo.toml", "[package]\nname = \"tool\"\n"),
        ]);
        let repo = Repo::from_dir(dir.path()).unwrap();
        let manifests = Manifests::load(&repo).unwrap();
        assert_eq!(manifests.workspaces[0].members, ["crates/a", "tool"]);
    }
}
