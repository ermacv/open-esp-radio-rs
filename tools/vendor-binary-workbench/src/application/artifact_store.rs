//! Command-scoped cache for immutable generated project artifacts.
//!
//! The store deliberately owns readers rather than decoded function bodies.
//! Indexed records and the call graph remain lazy. One most-recently-used
//! reader is retained so repeated focused queries share parsing without making
//! project-wide status retain every profile's indexes at once. Reloading a
//! [`ProjectSession`](super::ProjectSession) drops the store, so generated
//! artifacts can never remain stale across an explicit reload.

use std::{
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use crate::{Result, artifacts::LinkedIrReader};

#[derive(Default)]
pub(crate) struct ProjectArtifactStore {
    linked_ir: Mutex<Option<(PathBuf, Arc<LinkedIrReader>)>>,
}

impl ProjectArtifactStore {
    pub(super) fn linked_ir(&self, path: &Path) -> Result<Arc<LinkedIrReader>> {
        let mut reader = self
            .linked_ir
            .lock()
            .map_err(|_| crate::Error::invalid("project artifact store lock was poisoned"))?;
        if let Some((cached_path, cached_reader)) = reader.as_ref()
            && cached_path == path
        {
            return Ok(Arc::clone(cached_reader));
        }
        // Release a different profile before parsing the next one. A project
        // may contain several very large indexes; retaining every reader made
        // a cheap status query consume their aggregate memory.
        *reader = None;
        let loaded = Arc::new(LinkedIrReader::open(path)?);
        *reader = Some((path.to_owned(), Arc::clone(&loaded)));
        Ok(loaded)
    }
}
