//! Recursive discovery for the versioned scenario catalog.
//!
//! Scenario identity is independent of its domain folder. Entries are regular
//! TOML files or README.md; symlinks and other file kinds fail closed.

use std::{collections::BTreeSet, fs, path::Path};

use super::{Scenario, ScenarioFamily};
use crate::Result;

#[derive(Debug)]
pub struct Catalog<F> {
    scenarios: Vec<Scenario<F>>,
}

impl<F: ScenarioFamily> Catalog<F> {
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
                let scenario = Scenario::<F>::from_toml(&text, &path)?;
                if path.file_stem().and_then(|name| name.to_str()) != Some(scenario.id()) {
                    return Err(format!(
                        "scenario filename does not match ID `{}`: {}",
                        scenario.id(),
                        path.display()
                    )
                    .into());
                }
                if !ids.insert(scenario.id().to_owned()) {
                    return Err(format!("duplicate HIL scenario id `{}`", scenario.id()).into());
                }
                scenarios.push(scenario);
            }
        }
        if scenarios.is_empty() {
            return Err(format!("scenario catalog is empty: {}", directory.display()).into());
        }
        // Order by identity, independent of domain-folder placement.
        scenarios.sort_by(|left, right| left.id().cmp(right.id()));
        Self::new(scenarios)
    }

    /// Build a catalog from validated scenarios and check every
    /// experiment/control relation.
    pub fn new(scenarios: Vec<Scenario<F>>) -> Result<Self> {
        let catalog = Self { scenarios };
        for experiment in catalog.all() {
            let Some(control) = experiment.control() else {
                continue;
            };
            let control = catalog.get(control)?;
            if control.control().is_some() {
                return Err(format!(
                    "{}: control {} must be an independent scenario",
                    experiment.id(),
                    control.id()
                )
                .into());
            }
            experiment
                .family
                .validate_control(&control.family)
                .map_err(|error| {
                    format!("{}: control {}: {error}", experiment.id(), control.id())
                })?;
        }
        Ok(catalog)
    }

    pub fn all(&self) -> &[Scenario<F>] {
        &self.scenarios
    }

    pub fn get(&self, id: &str) -> Result<&Scenario<F>> {
        self.scenarios
            .iter()
            .find(|scenario| scenario.id() == id)
            .ok_or_else(|| format!("unknown HIL scenario `{id}`").into())
    }
}

#[cfg(test)]
mod tests;
