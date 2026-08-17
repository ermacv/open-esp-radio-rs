//! Composition of imported SVD catalogs and the reviewed register model.

use std::path::PathBuf;

use crate::{MmioMap, ProjectSpec, Result};

pub(super) fn load(
    enabled: bool,
    svd_paths: &[PathBuf],
    project: Option<&ProjectSpec>,
) -> Result<MmioMap> {
    if enabled {
        crate::register_catalog::load(svd_paths, project)
    } else {
        MmioMap::load_all(&[]).map_err(Into::into)
    }
}
