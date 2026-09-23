//! Loading immutable JSON emitted by `interfaces discover`.

use std::{fs, path::Path};

use crate::{Result, error::BlobrayError};

mod parse;
mod validate;

pub(crate) use validate::validate_sha256;

pub(crate) use super::observations::*;

impl InterfaceFacts {
    pub(crate) fn from_document(document: crate::artifacts::StoredInterfaceFacts) -> Result<Self> {
        parse::from_document(document)
    }

    pub(crate) fn validate(&self) -> Result<()> {
        validate::validate(self)
    }

    #[tracing::instrument(name = "load_interface_facts", fields(path = %path.display()))]
    pub(crate) fn load(path: &Path) -> Result<Self> {
        let input = fs::read_to_string(path)
            .map_err(|error| crate::Error::read("interface discovery report", path, error))?;
        Self::parse(path, &input)
    }

    pub(crate) fn parse(path: &Path, input: &str) -> Result<Self> {
        parse::parse(input).map_err(|error| {
            BlobrayError::manifest_document("interface discovery report", path, input, error)
        })
    }

    pub(crate) fn artifact(&self, index: usize) -> Option<&InterfaceFactArtifact> {
        self.artifacts
            .iter()
            .find(|artifact| artifact.index == index)
    }

    pub(crate) fn observed_slots(&self) -> usize {
        self.tables.iter().map(|table| table.slots.len()).sum()
    }

    pub(crate) const fn observed_calls(&self) -> usize {
        self.calls.len()
    }
}
