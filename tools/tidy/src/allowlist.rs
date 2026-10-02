//! The reviewed exceptions of the checks, each with a reason.
//!
//! An exception that no longer applies fails its check, so the list never
//! outlives what it excuses.

use toml::{Table, Value};

use crate::Result;

/// The allowlist's path, relative to the repository root.
pub const PATH: &str = "tools/tidy/allowlist.toml";

/// A Rust file that rustc compiles directly by path, outside any crate root.
#[derive(Clone, Debug)]
pub struct Uncompiled {
    pub path: String,
    pub reason: String,
}

/// A dependency the text check cannot see used.
#[derive(Clone, Debug)]
pub struct UnusedDependency {
    /// Repository path of the declaring `Cargo.toml`.
    pub manifest: String,
    pub dependency: String,
    pub reason: String,
}

#[derive(Clone, Debug, Default)]
pub struct Allowlist {
    pub uncompiled: Vec<Uncompiled>,
    pub unused_dependencies: Vec<UnusedDependency>,
}

fn field(entry: &Table, key: &str, section: &str) -> Result<String> {
    let value = entry
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| format!("{PATH}: every [[{section}]] entry needs a non-empty `{key}`"))?;
    Ok(value.to_owned())
}

fn entries<'a>(table: &'a Table, section: &str) -> Result<Vec<&'a Table>> {
    match table.get(section) {
        None => Ok(vec![]),
        Some(Value::Array(items)) => items
            .iter()
            .map(|item| {
                item.as_table()
                    .ok_or_else(|| format!("{PATH}: [[{section}]] entries must be tables"))
            })
            .collect(),
        Some(_) => Err(format!("{PATH}: `{section}` must be an array of tables")),
    }
}

impl Allowlist {
    pub fn parse(text: &str) -> Result<Self> {
        let table: Table = text
            .parse()
            .map_err(|error| format!("{PATH}: invalid TOML: {error}"))?;
        if let Some(unknown) = table
            .keys()
            .find(|key| !matches!(key.as_str(), "uncompiled" | "unused-dependency"))
        {
            return Err(format!("{PATH}: unknown section `{unknown}`"));
        }
        let uncompiled = entries(&table, "uncompiled")?
            .into_iter()
            .map(|entry| {
                Ok(Uncompiled {
                    path: field(entry, "path", "uncompiled")?,
                    reason: field(entry, "reason", "uncompiled")?,
                })
            })
            .collect::<Result<_>>()?;
        let unused_dependencies = entries(&table, "unused-dependency")?
            .into_iter()
            .map(|entry| {
                Ok(UnusedDependency {
                    manifest: field(entry, "manifest", "unused-dependency")?,
                    dependency: field(entry, "dependency", "unused-dependency")?,
                    reason: field(entry, "reason", "unused-dependency")?,
                })
            })
            .collect::<Result<_>>()?;
        Ok(Self {
            uncompiled,
            unused_dependencies,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_entry_needs_a_reason() {
        let error = Allowlist::parse("[[uncompiled]]\npath = \"a.rs\"\n").unwrap_err();
        assert!(error.contains("reason"), "{error}");
        let list = Allowlist::parse(
            "[[uncompiled]]\npath = \"a.rs\"\nreason = \"fixture\"\n[[unused-dependency]]\nmanifest = \"Cargo.toml\"\ndependency = \"x\"\nreason = \"linker\"\n",
        )
        .unwrap();
        assert_eq!(list.uncompiled[0].path, "a.rs");
        assert_eq!(list.unused_dependencies[0].dependency, "x");
    }

    #[test]
    fn unknown_sections_are_refused() {
        assert!(Allowlist::parse("[[orphans]]\npath = \"a\"\nreason = \"b\"\n").is_err());
    }
}
