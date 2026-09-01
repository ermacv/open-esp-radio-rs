//! Filesystem-backed loading for the versioned scenario catalog.

use std::{collections::BTreeSet, fs, path::Path};

use super::{Catalog, Scenario};
use crate::Result;

impl Catalog {
    pub fn load(directory: &Path) -> Result<Self> {
        let mut paths = fs::read_dir(directory)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.retain(|path| {
            path.extension()
                .is_some_and(|extension| extension == "toml")
        });
        paths.sort();
        let mut scenarios = Vec::with_capacity(paths.len());
        let mut ids = BTreeSet::new();
        for path in paths {
            let text = fs::read_to_string(&path)?;
            let mut scenario: Scenario =
                toml::from_str(&text).map_err(|error| format!("{}: {error}", path.display()))?;
            scenario.source = path;
            scenario.validate()?;
            if !ids.insert(scenario.id.clone()) {
                return Err(format!("duplicate HIL scenario id `{}`", scenario.id).into());
            }
            scenarios.push(scenario);
        }
        if scenarios.is_empty() {
            return Err(format!("scenario catalog is empty: {}", directory.display()).into());
        }
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
