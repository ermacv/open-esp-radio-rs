//! Recursive discovery for the versioned scenario catalog.
//!
//! Scenario identity is independent of its domain folder. Entries are regular
//! TOML files or README.md; symlinks and other file kinds fail closed.

use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

use super::{HEADER_FIELDS, Header, Scenario, ScenarioFamily};
use crate::Result;

#[derive(Debug)]
pub struct Catalog<F> {
    scenarios: Vec<Scenario<F>>,
}

/// One scenario document of a catalog, read without its family's types:
/// its validated header and the whole document as a value.
#[derive(Clone, Debug)]
pub struct Document {
    pub path: PathBuf,
    pub header: Header,
    pub value: serde_json::Value,
}

/// The documents of the catalog below `directory`, ordered by id: the one
/// discovery of a catalog, which every reader shares. Each is a regular
/// TOML file named by its id, or a README.md; a symlink anywhere in the
/// path, another file kind, a duplicate id, a header outside this build's
/// schema or a document without exactly one family table fails closed.
pub fn documents(directory: &Path) -> Result<Vec<Document>> {
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
    let mut documents = Vec::new();
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
            let document =
                document(&text).map_err(|error| format!("{}: {error}", path.display()))?;
            if path.file_stem().and_then(|name| name.to_str()) != Some(document.0.id.as_str()) {
                return Err(format!(
                    "scenario filename does not match ID `{}`: {}",
                    document.0.id,
                    path.display()
                )
                .into());
            }
            if !ids.insert(document.0.id.clone()) {
                return Err(format!("duplicate HIL scenario id `{}`", document.0.id).into());
            }
            documents.push(Document {
                path,
                header: document.0,
                value: document.1,
            });
        }
    }
    if documents.is_empty() {
        return Err(format!("scenario catalog is empty: {}", directory.display()).into());
    }
    // Order by identity, independent of domain-folder placement.
    documents.sort_by(|left, right| left.header.id.cmp(&right.header.id));
    Ok(documents)
}

/// The validated header and value of one TOML scenario document, which must
/// hold exactly one table besides its header: its family's.
fn document(text: &str) -> Result<(Header, serde_json::Value)> {
    let table: toml::Table = toml::from_str(text)?;
    let value = serde_json::to_value(table)?;
    let serde_json::Value::Object(fields) = &value else {
        return Err("scenario document is not a table".into());
    };
    let header = fields
        .iter()
        .filter(|(key, _)| HEADER_FIELDS.contains(&key.as_str()))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect::<serde_json::Map<_, _>>();
    let header: Header = serde_json::from_value(serde_json::Value::Object(header))?;
    header.validate()?;
    let families = fields
        .iter()
        .filter(|(key, _)| !HEADER_FIELDS.contains(&key.as_str()))
        .collect::<Vec<_>>();
    if !matches!(families.as_slice(), [(_, serde_json::Value::Object(_))]) {
        return Err(format!(
            "scenario `{}` must name exactly one family table",
            header.id
        )
        .into());
    }
    Ok((header, value))
}

impl<F: ScenarioFamily> Catalog<F> {
    /// The catalog below `directory`, each document parsed and validated
    /// with its family's types.
    pub fn load(directory: &Path) -> Result<Self> {
        let scenarios = documents(directory)?
            .into_iter()
            .map(|document| {
                Scenario::<F>::from_value(document.value)
                    .and_then(|mut scenario| {
                        scenario.source = document.path.clone();
                        scenario.validate()?;
                        Ok(scenario)
                    })
                    .map_err(|error| format!("{}: {error}", document.path.display()).into())
            })
            .collect::<Result<Vec<_>>>()?;
        Ok(Self::new(scenarios))
    }

    /// Build a catalog from validated scenarios.
    pub fn new(scenarios: Vec<Scenario<F>>) -> Self {
        Self { scenarios }
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
