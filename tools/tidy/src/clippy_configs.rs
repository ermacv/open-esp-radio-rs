//! Copied `clippy.toml` files stay complete and identical.
//!
//! Clippy reads only the nearest `clippy.toml` (or `.clippy.toml`) above a
//! package, so a package's own file replaces the root one entirely. The
//! check keeps three rules:
//!
//! - every such file repeats each entry of every `disallowed-*` list of
//!   the root `clippy.toml`, by its path, so no package loses a
//!   repository-wide rule by adding its own; an entry may be a path string
//!   or a table with a `path`;
//! - files that declare one group on a line `# copies: <group>` are
//!   byte-identical, and a group has at least two files; a group's shared
//!   rule set is then changed in every copy at once;
//! - every HIL scenario package carries its own copy of the `hil-scenario`
//!   group. The packages at `hil/model`, `hil/host/executor` and under
//!   `hil/scenarios/` do (#761, owner decision 6221112155, until #761 step
//!   10 replaces the copies with one `hil/clippy.toml`); so does, by this
//!   check's own rule, any other package under `hil/` that depends on
//!   `oer-hil-model`. A consumer of the model outside `hil/`, such as the
//!   qualification evaluator, is not a scenario package.

use std::collections::{BTreeMap, BTreeSet};

use crate::Context;

const ROOT: &str = "clippy.toml";
const FILES: [&str; 2] = ["clippy.toml", ".clippy.toml"];
const GROUP: &str = "# copies: ";
const HIL_SCENARIO: &str = "hil-scenario";
const HIL_SCENARIO_PATHS: [&str; 2] = ["hil/model", "hil/host/executor"];
const HIL_SCENARIO_TREE: &str = "hil/scenarios/";
const HIL_MODEL: &str = "oer-hil-model";
/// Only HIL packages are scenario packages: a consumer of the model outside
/// `hil/`, such as the qualification evaluator, keeps its own rules.
const HIL_TREE: &str = "hil/";

/// The paths of every `disallowed-*` list of a clippy configuration, by
/// list name.
fn disallowed(text: &str) -> Result<BTreeMap<String, BTreeSet<String>>, String> {
    let table: toml::Table = toml::from_str(text).map_err(|error| error.to_string())?;
    let mut lists = BTreeMap::new();
    for (key, value) in table
        .iter()
        .filter(|(key, _)| key.starts_with("disallowed-"))
    {
        let entries = value
            .as_array()
            .ok_or_else(|| format!("`{key}` is not an array"))?;
        let mut paths = BTreeSet::new();
        for entry in entries {
            let path = match entry {
                toml::Value::String(path) => path.as_str(),
                toml::Value::Table(table) => table
                    .get("path")
                    .and_then(toml::Value::as_str)
                    .ok_or_else(|| format!("an entry of `{key}` has no `path`"))?,
                _ => return Err(format!("an entry of `{key}` is neither a path nor a table")),
            };
            paths.insert(path.to_owned());
        }
        lists.insert(key.clone(), paths);
    }
    Ok(lists)
}

fn group(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.strip_prefix(GROUP))
        .map(str::trim)
}

fn config(file: &str) -> bool {
    file.rsplit('/')
        .next()
        .is_some_and(|name| FILES.contains(&name))
}

pub fn check(context: &Context<'_>) -> Vec<String> {
    let repo = context.repo;
    let mut problems = Vec::new();
    let root = match repo
        .read(ROOT)
        .map_err(|error| error.to_string())
        .and_then(|text| disallowed(&text))
    {
        Ok(root) => root,
        Err(error) => return vec![format!("{ROOT}: {error}")],
    };
    let mut groups: BTreeMap<String, Vec<(String, String)>> = BTreeMap::new();
    for file in repo.files().filter(|file| *file != ROOT && config(file)) {
        let text = match repo.read(file) {
            Ok(text) => text,
            Err(error) => {
                problems.push(format!("{file}: {error}"));
                continue;
            }
        };
        match disallowed(&text) {
            Ok(lists) => {
                for (list, paths) in &root {
                    for path in paths {
                        if !lists.get(list).is_some_and(|own| own.contains(path)) {
                            problems.push(format!(
                                "{file}: does not repeat the root `{list}` entry for `{path}`; the nearest clippy configuration replaces the root one"
                            ));
                        }
                    }
                }
            }
            Err(error) => problems.push(format!("{file}: {error}")),
        }
        if let Some(name) = group(&text) {
            groups
                .entry(name.to_owned())
                .or_default()
                .push((file.to_owned(), text));
        }
    }
    for (name, files) in &groups {
        let [(first, text), rest @ ..] = files.as_slice() else {
            continue;
        };
        if rest.is_empty() {
            problems.push(format!(
                "{first}: the copy group `{name}` has no other copy"
            ));
        }
        for (file, other) in rest {
            if other != text {
                problems.push(format!(
                    "{file}: differs from {first}, a copy of the same group `{name}`"
                ));
            }
        }
    }
    let copies: BTreeSet<&str> = groups
        .get(HIL_SCENARIO)
        .into_iter()
        .flatten()
        .filter_map(|(file, _)| file.rsplit_once('/').map(|(directory, _)| directory))
        .collect();
    for package in context.model.packages() {
        let scenario = HIL_SCENARIO_PATHS.contains(&package.directory.as_str())
            || package.directory.starts_with(HIL_SCENARIO_TREE)
            || package.directory.starts_with(HIL_TREE)
                && package
                    .dependencies
                    .iter()
                    .any(|dependency| dependency.package == HIL_MODEL);
        if scenario && !copies.contains(package.directory.as_str()) {
            problems.push(format!(
                "{}: the HIL scenario package {} has no clippy.toml of the `{HIL_SCENARIO}` copy group",
                package.directory, package.name
            ));
        }
    }
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::problems;

    const ROOT_RULES: &str = "disallowed-methods = [\n    { path = \"core::hint::spin_loop\", reason = \"yield\" },\n]\ndisallowed-types = [\"std::sync::Mutex\"]\n";

    fn copy(extra: &str) -> String {
        format!(
            "# copies: hil-scenario\ndisallowed-methods = [\n    {{ path = \"core::hint::spin_loop\", reason = \"yield\" }},\n{extra}]\ndisallowed-types = [\"std::sync::Mutex\"]\n"
        )
    }

    fn manifest(name: &str, dependencies: &str) -> String {
        format!(
            "[package]\nname = \"{name}\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[dependencies]\n{dependencies}"
        )
    }

    fn workspace(members: &[&str]) -> String {
        let members: Vec<String> = members
            .iter()
            .map(|member| format!("\"{member}\""))
            .collect();
        format!(
            "[workspace]\nresolver = \"3\"\nmembers = [{}]\n",
            members.join(", ")
        )
    }

    fn run(files: &[(&str, &str)]) -> Vec<String> {
        problems(files, check)
    }

    #[test]
    fn identical_complete_copies_pass() {
        let a = copy("    \"std::thread::sleep\",\n");
        let model = manifest("oer-hil-model", "");
        let executor = manifest(
            "oer-hil-executor",
            "oer-hil-model = { path = \"../../model\" }\n",
        );
        assert_eq!(
            run(&[
                (
                    "Cargo.toml",
                    &workspace(&["hil/model", "hil/host/executor"])
                ),
                ("clippy.toml", ROOT_RULES),
                ("hil/model/Cargo.toml", &model),
                ("hil/model/src/lib.rs", ""),
                ("hil/model/clippy.toml", &a),
                ("hil/host/executor/Cargo.toml", &executor),
                ("hil/host/executor/src/lib.rs", ""),
                ("hil/host/executor/.clippy.toml", &a),
            ]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_copy_that_drops_a_root_rule_of_any_list_fails() {
        let file = "disallowed-methods = [\"core::hint::spin_loop\"]\n";
        let problems = run(&[("clippy.toml", ROOT_RULES), ("a/clippy.toml", file)]);
        assert_eq!(problems.len(), 1, "{problems:?}");
        assert!(
            problems[0].contains("disallowed-types") && problems[0].contains("std::sync::Mutex"),
            "{problems:?}"
        );
    }

    #[test]
    fn diverged_or_lonely_copies_fail() {
        let a = copy("    { path = \"std::thread::sleep\", reason = \"derive\" },\n");
        let b = copy("");
        let problems = run(&[
            ("clippy.toml", ROOT_RULES),
            ("a/clippy.toml", &a),
            ("b/clippy.toml", &b),
        ]);
        assert_eq!(problems, vec!["b/clippy.toml: differs from a/clippy.toml, a copy of the same group `hil-scenario`".to_owned()]);
        let lonely = run(&[("clippy.toml", ROOT_RULES), ("a/clippy.toml", &a)]);
        assert_eq!(
            lonely,
            vec!["a/clippy.toml: the copy group `hil-scenario` has no other copy".to_owned()]
        );
    }

    #[test]
    fn a_hil_scenario_package_without_its_copy_fails() {
        let a = copy("");
        let model = manifest("oer-hil-model", "");
        let block = manifest(
            "oer-hil-scenarios-toy",
            "oer-hil-model = { path = \"../../model\" }\n",
        );
        let problems = run(&[
            (
                "Cargo.toml",
                &workspace(&[
                    "hil/model",
                    "hil/scenarios/toy",
                    "tools/other",
                    "qualification/evaluator",
                ]),
            ),
            ("clippy.toml", ROOT_RULES),
            ("hil/model/Cargo.toml", &model),
            ("hil/model/src/lib.rs", ""),
            ("hil/model/clippy.toml", &a),
            ("hil/scenarios/toy/Cargo.toml", &block),
            ("hil/scenarios/toy/src/lib.rs", ""),
            (
                "tools/other/Cargo.toml",
                &manifest(
                    "oer-other",
                    "oer-hil-model = { path = \"../../hil/model\" }\n",
                ),
            ),
            ("tools/other/src/lib.rs", ""),
            ("tools/other/clippy.toml", &a),
            // A consumer of the model outside hil/ needs no copy.
            (
                "qualification/evaluator/Cargo.toml",
                &manifest(
                    "oer-qualification",
                    "oer-hil-model = { path = \"../../hil/model\" }\n",
                ),
            ),
            ("qualification/evaluator/src/lib.rs", ""),
        ]);
        assert_eq!(
            problems,
            vec![
                "hil/scenarios/toy: the HIL scenario package oer-hil-scenarios-toy has no clippy.toml of the `hil-scenario` copy group"
                    .to_owned()
            ]
        );
    }
}
