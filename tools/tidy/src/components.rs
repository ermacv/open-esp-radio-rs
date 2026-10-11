//! The documents that describe the dependency table of Blobray and the shared
//! RV32 layer state exactly `oer_repo::policy::COMPONENTS`.
//!
//! Each document holds a markdown table under its heading whose first column
//! names packages and whose last column names the packages they may depend
//! on, every name in backticks (`none` for an empty row). A documented row
//! that differs from the table, a row the document owns but leaves out, or a
//! package the table does not know fails the check.

use std::collections::{BTreeMap, BTreeSet};

use crate::{Context, Result};

/// A document, the heading of its table and the package-name prefix of the
/// rows it owns.
pub const DOCUMENTS: &[(&str, &str, &str)] = &[
    (
        "tools/blobray/docs/design/architecture.md",
        "## Crates and modules",
        "blobray-",
    ),
    ("tools/riscv/README.md", "## Dependency table", "oer-"),
];

pub fn check(context: &Context<'_>) -> Result<Vec<String>> {
    let mut texts = Vec::new();
    for (path, _, _) in DOCUMENTS {
        texts.push(context.repo.read(path)?);
    }
    let documents: Vec<_> = DOCUMENTS
        .iter()
        .zip(&texts)
        .map(|(&(path, heading, prefix), text)| (path, heading, prefix, text.as_str()))
        .collect();
    Ok(check_documents(oer_repo::policy::COMPONENTS, &documents))
}

/// The problems of `documents` (path, heading, owned prefix, text) against
/// `table`.
pub fn check_documents(
    table: &[(&str, &[&str])],
    documents: &[(&str, &str, &str, &str)],
) -> Vec<String> {
    let table: BTreeMap<&str, BTreeSet<&str>> = table
        .iter()
        .map(|(name, allowed)| (*name, allowed.iter().copied().collect()))
        .collect();
    let mut problems = Vec::new();
    for &(path, heading, prefix, text) in documents {
        let rows = rows(text, heading);
        if rows.is_empty() {
            problems.push(format!("{path}: no dependency table under `{heading}`"));
            continue;
        }
        let mut documented = BTreeSet::new();
        for (packages, allowed) in &rows {
            for package in packages {
                documented.insert(package.as_str());
                let Some(expected) = table.get(package.as_str()) else {
                    problems.push(format!(
                        "{path}: {package} has no row in oer_repo::policy::COMPONENTS"
                    ));
                    continue;
                };
                let stated: BTreeSet<&str> = allowed.iter().map(String::as_str).collect();
                if &stated != expected {
                    problems.push(format!(
                        "{path}: {package} may depend on {} by oer_repo::policy::COMPONENTS; the document says {}",
                        list(expected),
                        list(&stated)
                    ));
                }
            }
        }
        for package in table.keys().filter(|name| name.starts_with(prefix)) {
            if !documented.contains(package) {
                problems.push(format!(
                    "{path}: {package} of oer_repo::policy::COMPONENTS is missing from the table"
                ));
            }
        }
    }
    problems
}

fn list(names: &BTreeSet<&str>) -> String {
    if names.is_empty() {
        "none".to_owned()
    } else {
        names.iter().copied().collect::<Vec<_>>().join(", ")
    }
}

/// The rows of the first markdown table after `heading` and before the next
/// heading: the backticked names of the first and the last column. The header
/// and separator rows name nothing and are skipped.
fn rows(text: &str, heading: &str) -> Vec<(Vec<String>, Vec<String>)> {
    let mut lines = text.lines().skip_while(|line| line.trim_end() != heading);
    if lines.next().is_none() {
        return Vec::new();
    }
    let mut rows = Vec::new();
    for line in lines.take_while(|line| !line.starts_with('#')) {
        let line = line.trim();
        if !line.starts_with('|') {
            if !rows.is_empty() {
                break;
            }
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').collect();
        let (Some(first), Some(last)) = (cells.first(), cells.last()) else {
            continue;
        };
        let packages = backticked(first);
        if packages.is_empty() {
            continue;
        }
        rows.push((packages, backticked(last)));
    }
    rows
}

/// Every name in backticks in `cell`.
fn backticked(cell: &str) -> Vec<String> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .map(str::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const TABLE: &[(&str, &[&str])] = &[
        ("oer-a", &[]),
        ("oer-b", &["oer-a"]),
        ("blobray-c", &["oer-a", "oer-b"]),
    ];

    fn document(rows: &str) -> String {
        format!(
            "# Title\n\n## Table\n\n| Package | Owns | May depend on |\n| --- | --- | --- |\n{rows}\n## Next\n\n| `oer-z` | x | `oer-a` |\n"
        )
    }

    fn problems(rows: &str, prefix: &str) -> Vec<String> {
        let text = document(rows);
        check_documents(TABLE, &[("doc.md", "## Table", prefix, &text)])
    }

    #[test]
    fn a_document_that_states_the_table_passes() {
        assert!(problems("| `oer-a` | a | none |\n| `oer-b` | b | `oer-a` |", "oer-").is_empty());
        // Several packages may share a row; order does not matter.
        assert!(problems("| `blobray-c` | c | `oer-b`, `oer-a` |", "blobray-").is_empty());
    }

    #[test]
    fn a_row_that_differs_is_missing_or_unknown_fails() {
        assert_eq!(
            problems("| `oer-a` | a | none |\n| `oer-b` | b | none |", "oer-"),
            [
                "doc.md: oer-b may depend on oer-a by oer_repo::policy::COMPONENTS; the document says none"
            ]
        );
        assert_eq!(
            problems("| `oer-a` | a | none |", "oer-"),
            ["doc.md: oer-b of oer_repo::policy::COMPONENTS is missing from the table"]
        );
        assert_eq!(
            problems(
                "| `oer-a` | a | none |\n| `oer-b` | b | `oer-a` |\n| `oer-x` | x | none |",
                "oer-"
            ),
            ["doc.md: oer-x has no row in oer_repo::policy::COMPONENTS"]
        );
        // The table under the next heading is never read.
        assert!(
            !problems("| `oer-a` | a | none |\n| `oer-b` | b | `oer-a` |", "oer-")
                .iter()
                .any(|problem| problem.contains("oer-z"))
        );
    }

    #[test]
    fn a_document_without_its_table_fails() {
        assert_eq!(
            check_documents(TABLE, &[("doc.md", "## Missing", "oer-", "# Title\n")]),
            ["doc.md: no dependency table under `## Missing`"]
        );
    }
}
