//! Composition of imported SVD catalogs and the reviewed register model.

use std::path::PathBuf;

use crate::{MmioRegisterMap, ProjectSpec, Result};

use super::super::args::Command;

pub(super) fn load(
    command: Command,
    svd_paths: &[PathBuf],
    project: Option<&ProjectSpec>,
) -> Result<MmioRegisterMap> {
    let mut svd = if command.uses_register_catalog() {
        MmioRegisterMap::load_all(svd_paths)?
    } else {
        MmioRegisterMap::load_all(&[])?
    };
    if command.uses_register_catalog()
        && let Some(paths) = project.and_then(|project| project.registers.as_ref())
        && paths.model.is_file()
        && crate::registers::RegisterModel::is_model_file(&paths.model)?
    {
        let model = crate::registers::RegisterModel::load(&paths.model)?;
        let (model_svd, _) = model.render_svd()?;
        svd.merge(MmioRegisterMap::parse(&model_svd)?)?;
    }
    Ok(svd)
}
