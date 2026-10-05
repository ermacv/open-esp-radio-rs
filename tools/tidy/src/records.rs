//! Repository paths named by tracked qualification and evidence records.
//!
//! The keys mirror the record schemas of the qualification evaluator
//! (`qualification/evaluator/src/model*`, `hil/shard.rs`) and
//! the vendor evidence index (`oer-vendor-evidence`, `verification/evidence`):
//!
//! - program manifests (`qualification/targets/**.toml`): `catalogs`, and
//!   `[hil] catalog`;
//! - catalogs (`qualification/catalog/**.toml`): `imports`,
//!   `[validation] hil-catalog`, capability `vendor-anchors`,
//!   `source-document`, `documents`, `source-paths`,
//!   `[development] knowledge` and host-test `manifest`/`source`; every
//!   `packages` entry names a package of the repository;
//! - evidence shards (`verification/*/evidence/**.json` and each directory a
//!   program names as its vendor `evidence-index` or HIL `evidence`):
//!   `sources[].path`.
//!
//! Output directories (`evidence-index`, `[hil] evidence`, `[hil] runs`) are
//! where tools write and need not exist.

use std::collections::BTreeSet;

use serde_json::Value as Json;
use toml::{Table, Value};

use oer_repo::manifest::strings;

use crate::{Context, Result};

/// Keys whose string or string-array value names repository paths, in any
/// table of a qualification record.
const PATH_KEYS: &[&str] = &[
    "catalogs",
    "imports",
    "hil-catalog",
    "vendor-anchors",
    "source-document",
    "documents",
    "source-paths",
    "knowledge",
];

/// Keys that name paths only inside the named parent table or array.
const SCOPED_PATH_KEYS: &[(&str, &str)] = &[
    ("hil", "catalog"),
    ("host-tests", "manifest"),
    ("host-tests", "source"),
];

/// Output directory keys a program names, holding evidence shards.
const EVIDENCE_DIRECTORIES: &[(&str, &str)] =
    &[("verification", "evidence-index"), ("hil", "evidence")];

struct Walk<'a> {
    context: &'a Context<'a>,
    file: &'a str,
    packages: &'a BTreeSet<&'a str>,
    problems: Vec<String>,
}

impl Walk<'_> {
    fn path(&mut self, key: &str, path: &str) {
        if !self.context.repo.exists(path) {
            self.problems
                .push(format!("{}: `{key}` names missing {path}", self.file));
        }
    }

    fn table(&mut self, parent: &str, table: &Table) {
        for (key, value) in table {
            if PATH_KEYS.contains(&key.as_str())
                || SCOPED_PATH_KEYS.contains(&(parent, key.as_str()))
            {
                for path in strings(Some(value)) {
                    self.path(key, &path);
                }
            } else if key == "packages" {
                for package in strings(Some(value)) {
                    if !self.packages.contains(package.as_str()) {
                        self.problems.push(format!(
                            "{}: `packages` names unknown package {package}",
                            self.file
                        ));
                    }
                }
            }
            match value {
                Value::Table(child) => self.table(key, child),
                Value::Array(items) => {
                    for item in items {
                        if let Value::Table(child) = item {
                            self.table(key, child);
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn is_record(file: &str) -> bool {
    file.ends_with(".toml")
        && ["qualification/targets/", "qualification/catalog/"]
            .iter()
            .any(|prefix| file.starts_with(prefix))
}

/// Every repository path a record names exists.
pub fn check(context: &Context<'_>) -> Result<Vec<String>> {
    let packages: BTreeSet<&str> = context
        .model
        .packages()
        .iter()
        .map(|package| package.name.as_str())
        .collect();
    let mut evidence: BTreeSet<String> = context
        .repo
        .files()
        .filter(|file| file.starts_with("verification/"))
        .filter(|file| file.split('/').nth(2) == Some("evidence"))
        .filter(|file| file.ends_with(".json"))
        .map(str::to_owned)
        .collect();
    let mut problems = vec![];
    for file in context.repo.files().filter(|file| is_record(file)) {
        let table: Table = context
            .repo
            .read(file)?
            .parse()
            .map_err(|error| format!("{file}: invalid TOML: {error}"))?;
        let mut walk = Walk {
            context,
            file,
            packages: &packages,
            problems: vec![],
        };
        walk.table("", &table);
        problems.extend(walk.problems);
        if file.starts_with("qualification/targets/") {
            for (section, key) in EVIDENCE_DIRECTORIES {
                if let Some(directory) = table
                    .get(*section)
                    .and_then(Value::as_table)
                    .and_then(|section| section.get(*key))
                    .and_then(Value::as_str)
                {
                    evidence.extend(
                        context
                            .repo
                            .below(directory)
                            .filter(|file| file.ends_with(".json"))
                            .map(str::to_owned),
                    );
                }
            }
        }
    }
    for shard in &evidence {
        let value: Json = serde_json::from_str(&context.repo.read(shard)?)
            .map_err(|error| format!("{shard}: invalid JSON: {error}"))?;
        for source in value
            .get("sources")
            .and_then(Json::as_array)
            .into_iter()
            .flatten()
        {
            let Some(path) = source.get("path").and_then(Json::as_str) else {
                problems.push(format!("{shard}: a `sources` entry has no path"));
                continue;
            };
            if !context.repo.exists(path) {
                problems.push(format!("{shard}: source {path} does not exist"));
            }
        }
    }
    problems.sort();
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::problems;

    fn run(files: &[(&str, &str)]) -> Vec<String> {
        problems(files, |context| check(context).unwrap())
    }

    #[test]
    fn existing_paths_and_known_packages_pass() {
        let found = run(&[
            ("p/Cargo.toml", "[package]\nname = \"oer-p\"\n"),
            ("p/src/lib.rs", ""),
            ("docs/a.md", ""),
            (
                "qualification/catalog/c.toml",
                "[[capabilities]]\ndocuments = [\"docs/a.md\"]\nsource-paths = [\"p/src\"]\npackages = [\"oer-p\"]\n",
            ),
            (
                "qualification/targets/t.toml",
                "catalogs = [\"qualification/catalog/c.toml\"]\n[verification]\nevidence-index = \"not/yet/written\"\n[hil]\ncatalog = \"docs\"\nruns = \"target/runs\"\n",
            ),
        ]);
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn missing_record_paths_and_unknown_packages_fail() {
        let found = run(&[(
            "qualification/catalog/c.toml",
            "imports = [\"qualification/catalog/gone.toml\"]\n[[capabilities]]\npackages = [\"oer-gone\"]\n[capabilities.development]\nhost-tests = [{ manifest = \"x/Cargo.toml\", filter = \"f\", source = \"x/src/lib.rs\" }]\n",
        )]);
        assert_eq!(
            found,
            [
                "qualification/catalog/c.toml: `imports` names missing qualification/catalog/gone.toml",
                "qualification/catalog/c.toml: `manifest` names missing x/Cargo.toml",
                "qualification/catalog/c.toml: `packages` names unknown package oer-gone",
                "qualification/catalog/c.toml: `source` names missing x/src/lib.rs",
            ]
        );
    }

    #[test]
    fn evidence_shards_bound_to_missing_sources_fail() {
        let shard =
            r#"{"sources": [{"path": "a.rs", "sha256": "0"}, {"path": "gone.rs", "sha256": "0"}]}"#;
        let found = run(&[
            ("a.rs", ""),
            ("verification/chip/evidence/scenarios/s.json", shard),
            ("hil/evidence/chip/h.json", shard),
            (
                "qualification/targets/t.toml",
                "[hil]\nevidence = \"hil/evidence/chip\"\n",
            ),
        ]);
        assert_eq!(
            found,
            [
                "hil/evidence/chip/h.json: source gone.rs does not exist",
                "verification/chip/evidence/scenarios/s.json: source gone.rs does not exist",
            ]
        );
    }
}
