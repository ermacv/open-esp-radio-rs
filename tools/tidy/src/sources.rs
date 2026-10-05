//! Rust sources: every file belongs to a crate, and the markers other
//! tools trust sit only in compiled files.

use crate::{Context, Result};

/// Every Rust file is reachable from some package's crate roots, every
/// module declaration names a file, every explicit target exists, and every
/// allowlisted uncompiled file exists and is not reachable after all.
pub fn orphans(context: &Context<'_>) -> Vec<String> {
    let mut problems = vec![];
    let allowed = |path: &str| {
        context
            .allowlist
            .uncompiled
            .iter()
            .any(|entry| entry.path == path)
    };
    for file in context.repo.files().filter(|file| file.ends_with(".rs")) {
        if !context.reachable.contains(file) && !allowed(file) {
            problems.push(format!("{file}: reachable from no crate root"));
        }
    }
    for entry in &context.allowlist.uncompiled {
        if !context.repo.is_file(&entry.path) {
            problems.push(format!(
                "{}: allowlisted as uncompiled but does not exist",
                entry.path
            ));
        } else if context.reachable.contains(&entry.path) {
            problems.push(format!(
                "{}: allowlisted as uncompiled but a crate root reaches it",
                entry.path
            ));
        }
    }
    for package in context.model.packages() {
        for root in &package.missing_roots {
            problems.push(format!(
                "{}: target {root} does not exist",
                package.manifest
            ));
        }
    }
    for reach in context.reach.values() {
        for (file, declaration) in &reach.unresolved {
            problems.push(format!("{file}: `{declaration}` names no file"));
        }
    }
    problems.sort();
    problems.dedup();
    problems
}

/// A `// CAPABILITY:` anchor or a vendor `SOURCE` citation marker on a
/// comment line, by the recognisers of their owners.
fn marker(line: &str) -> Option<&'static str> {
    use oer_vendor_provenance::citation::{Syntax, is_marker_line};
    if crate::anchors::capability(line).is_some() {
        Some("CAPABILITY anchor")
    } else if is_marker_line(line, Syntax::Rust) {
        Some("vendor SOURCE citation")
    } else {
        None
    }
}

/// Capability anchors and vendor citations appear only in files a crate
/// root reaches: the qualification evaluator and the provenance check
/// trust them as statements about compiled code.
pub fn markers(context: &Context<'_>) -> Result<Vec<String>> {
    let mut problems = vec![];
    for file in context.repo.files().filter(|file| file.ends_with(".rs")) {
        if context.reachable.contains(file) {
            continue;
        }
        let text = context.repo.read(file)?;
        for (index, line) in text.lines().enumerate() {
            if let Some(kind) = marker(line) {
                problems.push(format!(
                    "{file}:{}: {kind} in a file no crate root reaches",
                    index + 1
                ));
            }
        }
    }
    Ok(problems)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::problems;

    const PACKAGE: (&str, &str) = ("p/Cargo.toml", "[package]\nname = \"p\"\n");

    #[test]
    fn a_reachable_file_passes_and_an_orphan_fails() {
        let found = problems(
            &[
                PACKAGE,
                ("p/src/lib.rs", "mod used;"),
                ("p/src/used.rs", ""),
                ("p/src/stale.rs", ""),
            ],
            orphans,
        );
        assert_eq!(found, ["p/src/stale.rs: reachable from no crate root"]);
    }

    #[test]
    fn allowlisted_files_must_exist_and_stay_uncompiled() {
        let allowlist = "[[uncompiled]]\npath = \"p/tests/fixtures/f.rs\"\nreason = \"compiled by a test\"\n\
                         [[uncompiled]]\npath = \"p/gone.rs\"\nreason = \"removed\"\n\
                         [[uncompiled]]\npath = \"p/src/lib.rs\"\nreason = \"stale\"\n";
        let found = problems(
            &[
                PACKAGE,
                ("p/src/lib.rs", ""),
                ("p/tests/fixtures/f.rs", ""),
                ("tools/tidy/allowlist.toml", allowlist),
            ],
            orphans,
        );
        assert_eq!(
            found,
            [
                "p/gone.rs: allowlisted as uncompiled but does not exist",
                "p/src/lib.rs: allowlisted as uncompiled but a crate root reaches it",
            ]
        );
    }

    #[test]
    fn dangling_declarations_and_targets_fail() {
        let found = problems(
            &[
                (
                    "p/Cargo.toml",
                    "[package]\nname = \"p\"\n[[bin]]\nname = \"b\"\npath = \"src/b.rs\"\n",
                ),
                ("p/src/lib.rs", "mod gone;"),
            ],
            orphans,
        );
        assert_eq!(
            found,
            [
                "p/Cargo.toml: target p/src/b.rs does not exist",
                "p/src/lib.rs: `mod gone;` names no file",
            ]
        );
    }

    #[test]
    fn markers_fail_only_outside_reachable_files() {
        let marked = "// CAPABILITY: wpa2\nfn f() {}\n// SOURCE(esp32s31): `libpp.a[x.o]::f`\n";
        let found = problems(
            &[
                PACKAGE,
                ("p/src/lib.rs", marked),
                ("p/stray.rs", marked),
                (
                    "p/notes.rs",
                    "// RESOURCE: not a citation\n// see SOURCES.md\n",
                ),
            ],
            |context| markers(context).unwrap(),
        );
        assert_eq!(
            found,
            [
                "p/stray.rs:1: CAPABILITY anchor in a file no crate root reaches",
                "p/stray.rs:3: vendor SOURCE citation in a file no crate root reaches",
            ]
        );
    }
}
