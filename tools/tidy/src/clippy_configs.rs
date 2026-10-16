//! Copied `clippy.toml` files stay complete and identical.
//!
//! Clippy reads only the nearest `clippy.toml` above a package, so a
//! package's own file replaces the root one entirely. The check keeps two
//! rules:
//!
//! - every `clippy.toml` repeats each `disallowed-methods` entry of the root
//!   `clippy.toml`, path and reason alike, so no package loses a
//!   repository-wide rule by adding its own;
//! - files that declare one group on a line `# copies: <group>` are
//!   byte-identical, and a group has at least two files; a group's shared
//!   rule set is then changed in every copy at once (for example the HIL
//!   scenario crates' `hil-scenario` group, until #761 step 10 replaces its
//!   copies with one `hil/clippy.toml`).

use std::collections::BTreeMap;

use oer_repo::Repo;

const ROOT: &str = "clippy.toml";
const FILE: &str = "clippy.toml";
const GROUP: &str = "# copies: ";

/// The `disallowed-methods` entries of a `clippy.toml`, as tables.
fn disallowed(text: &str) -> Result<Vec<toml::Table>, String> {
    let table: toml::Table = toml::from_str(text).map_err(|error| error.to_string())?;
    let Some(entries) = table.get("disallowed-methods") else {
        return Ok(Vec::new());
    };
    let entries = entries
        .as_array()
        .ok_or("`disallowed-methods` is not an array")?;
    entries
        .iter()
        .map(|entry| {
            entry
                .as_table()
                .cloned()
                .ok_or_else(|| "an entry is not a table".to_owned())
        })
        .collect()
}

fn group(text: &str) -> Option<&str> {
    text.lines()
        .find_map(|line| line.strip_prefix(GROUP))
        .map(str::trim)
}

pub fn check(repo: &Repo) -> Vec<String> {
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
    for file in repo
        .files()
        .filter(|file| *file != ROOT && file.rsplit('/').next() == Some(FILE))
    {
        let text = match repo.read(file) {
            Ok(text) => text,
            Err(error) => {
                problems.push(format!("{file}: {error}"));
                continue;
            }
        };
        match disallowed(&text) {
            Ok(entries) => {
                for entry in &root {
                    if !entries.contains(entry) {
                        let path = entry
                            .get("path")
                            .and_then(toml::Value::as_str)
                            .unwrap_or("?");
                        problems.push(format!(
                            "{file}: does not repeat the root `disallowed-methods` entry for `{path}`; the nearest clippy.toml replaces the root one"
                        ));
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
    for (name, files) in groups {
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
    problems
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::tree;

    const ROOT_RULES: &str = "disallowed-methods = [\n    { path = \"core::hint::spin_loop\", reason = \"yield\" },\n]\n";

    fn run(files: &[(&str, &str)]) -> Vec<String> {
        let dir = tree(files);
        check(&Repo::from_dir(dir.path()).unwrap())
    }

    fn copy(extra: &str) -> String {
        format!(
            "# copies: hil-scenario\ndisallowed-methods = [\n    {{ path = \"core::hint::spin_loop\", reason = \"yield\" }},\n{extra}]\n"
        )
    }

    #[test]
    fn identical_complete_copies_pass() {
        let a = copy("    { path = \"std::thread::sleep\", reason = \"derive\" },\n");
        assert_eq!(
            run(&[
                ("clippy.toml", ROOT_RULES),
                ("a/clippy.toml", &a),
                ("b/clippy.toml", &a)
            ]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn a_copy_that_drops_a_root_rule_fails() {
        let file = "disallowed-methods = [\n    { path = \"std::thread::sleep\", reason = \"derive\" },\n]\n";
        let problems = run(&[("clippy.toml", ROOT_RULES), ("a/clippy.toml", file)]);
        assert_eq!(problems.len(), 1);
        assert!(
            problems[0].contains("core::hint::spin_loop"),
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
}
