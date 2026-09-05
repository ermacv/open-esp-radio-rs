//! Recursive discovery for the versioned scenario catalog.
//!
//! Scenario identity is independent of its domain folder. Entries are regular
//! TOML files or README.md; symlinks and other file kinds fail closed.

use std::{collections::BTreeSet, fs, path::Path};

use super::{Catalog, Scenario};
use crate::Result;

impl Catalog {
    pub fn load(directory: &Path) -> Result<Self> {
        // Do not let a symlink in the supplied catalog path bypass the same
        // no-follow policy applied to entries below that directory.
        for component in directory
            .ancestors()
            .filter(|path| !path.as_os_str().is_empty())
        {
            if fs::symlink_metadata(component)?.file_type().is_symlink() {
                return Err(format!(
                    "scenario catalog path contains a symlink: {}",
                    component.display()
                )
                .into());
            }
        }
        if !fs::symlink_metadata(directory)?.file_type().is_dir() {
            return Err(format!(
                "scenario catalog is not a regular directory: {}",
                directory.display()
            )
            .into());
        }
        let mut scenarios = Vec::new();
        let mut pending = vec![directory.to_owned()];
        let mut ids = BTreeSet::new();
        while let Some(directory) = pending.pop() {
            let mut entries = fs::read_dir(directory)?.collect::<std::io::Result<Vec<_>>>()?;
            entries.sort_by_key(fs::DirEntry::file_name);
            for entry in entries {
                let path = entry.path();
                let kind = entry.file_type()?;
                if kind.is_dir() {
                    pending.push(path);
                    continue;
                }
                if !kind.is_file() {
                    return Err(format!(
                        "scenario catalog contains a non-regular entry: {}",
                        path.display()
                    )
                    .into());
                }
                if entry.file_name() == "README.md" {
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "toml") {
                    return Err(format!(
                        "scenario catalog contains an unsupported file: {}",
                        path.display()
                    )
                    .into());
                }
                let text = fs::read_to_string(&path)?;
                let mut scenario: Scenario = toml::from_str(&text)
                    .map_err(|error| format!("{}: {error}", path.display()))?;
                if path.file_stem().and_then(|name| name.to_str()) != Some(&scenario.id) {
                    return Err(format!(
                        "scenario filename does not match ID `{}`: {}",
                        scenario.id,
                        path.display()
                    )
                    .into());
                }
                scenario.source = path;
                scenario.validate()?;
                if !ids.insert(scenario.id.clone()) {
                    return Err(format!("duplicate HIL scenario id `{}`", scenario.id).into());
                }
                scenarios.push(scenario);
            }
        }
        if scenarios.is_empty() {
            return Err(format!("scenario catalog is empty: {}", directory.display()).into());
        }
        // Preserve the original flat filename order inside each ImageClass::ALL
        // group, regardless of future domain-folder placement.
        scenarios.sort_by(|left, right| left.id.cmp(&right.id));
        Ok(Self { scenarios })
    }

    pub fn all(&self) -> &[Scenario] {
        &self.scenarios
    }

    pub fn get(&self, id: &str) -> Result<&Scenario> {
        self.scenarios
            .iter()
            .find(|scenario| scenario.id == id)
            .ok_or_else(|| format!("unknown HIL scenario `{id}`").into())
    }
}

#[cfg(test)]
mod tests;
