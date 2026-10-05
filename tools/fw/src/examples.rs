//! The examples: each chip's example workspace (`examples/<chip>`), whose
//! members are the examples, found from the chip profiles and the
//! workspace manifests alone.

use std::path::{Path, PathBuf};

use oer_chip_profile::Profile;

use crate::Result;

/// One example: a member of its chip's example workspace.
#[derive(Clone, Debug)]
pub struct Example {
    /// Its directory name.
    pub name: String,
    pub profile: Profile,
    /// The workspace, relative to the repository root.
    pub workspace: PathBuf,
    pub package: String,
}

impl Example {
    /// `<chip>/<name>`, the name that is unique across chips.
    pub fn qualified(&self) -> String {
        format!("{}/{}", self.profile.id, self.name)
    }

    /// The directory of the example's bundles.
    pub fn directory(&self, root: &Path) -> PathBuf {
        root.join("target/firmware")
            .join(format!("{}-{}", self.profile.id, self.name))
    }
}

/// Every example of every chip, sorted by chip and name.
pub fn all(root: &Path) -> Result<Vec<Example>> {
    let mut examples = Vec::new();
    for profile in Profile::all(root)? {
        let workspace = Path::new("examples").join(&profile.id);
        let manifest = root.join(&workspace).join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        let table: toml::Table = toml::from_str(&std::fs::read_to_string(&manifest)?)
            .map_err(|error| format!("{}: {error}", manifest.display()))?;
        let members = table
            .get("workspace")
            .and_then(|workspace| workspace.get("members"))
            .and_then(toml::Value::as_array)
            .ok_or_else(|| format!("{} names no workspace members", manifest.display()))?;
        for member in members.iter().filter_map(toml::Value::as_str) {
            let member_manifest = root.join(&workspace).join(member).join("Cargo.toml");
            let member_table: toml::Table =
                toml::from_str(&std::fs::read_to_string(&member_manifest)?)
                    .map_err(|error| format!("{}: {error}", member_manifest.display()))?;
            let package = member_table
                .get("package")
                .and_then(|package| package.get("name"))
                .and_then(toml::Value::as_str)
                .ok_or_else(|| format!("{} has no package name", member_manifest.display()))?
                .to_owned();
            examples.push(Example {
                name: member.to_owned(),
                profile: profile.clone(),
                workspace: workspace.clone(),
                package,
            });
        }
    }
    Ok(examples)
}

/// The example `query` names: `<name>` when one chip has it, or
/// `<chip>/<name>`.
pub fn find(root: &Path, query: &str) -> Result<Example> {
    let examples = all(root)?;
    let matches = examples
        .iter()
        .filter(|example| example.name == query || example.qualified() == query)
        .collect::<Vec<_>>();
    match matches.as_slice() {
        [one] => Ok((*one).clone()),
        [] => Err(format!(
            "no example `{query}`; examples: {}",
            examples
                .iter()
                .map(Example::qualified)
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into()),
        _ => Err(format!(
            "`{query}` names an example of several chips; say which: {}",
            matches
                .iter()
                .map(|example| example.qualified())
                .collect::<Vec<_>>()
                .join(", ")
        )
        .into()),
    }
}
