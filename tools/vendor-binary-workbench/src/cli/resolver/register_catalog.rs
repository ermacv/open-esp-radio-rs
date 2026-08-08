//! Composition of imported SVD catalogs and the reviewed register model.

use std::path::PathBuf;

use crate::{MmioMap, ProjectSpec, Result};

use super::super::args::Command;

pub(super) fn load(
    command: Command,
    svd_paths: &[PathBuf],
    project: Option<&ProjectSpec>,
) -> Result<MmioMap> {
    if command.uses_register_catalog() {
        crate::register_catalog::load(svd_paths, project)
    } else {
        MmioMap::load_all(&[]).map_err(Into::into)
    }
}
