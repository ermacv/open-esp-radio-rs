//! Reusable composition of imported SVD and reviewed register-model data.

use std::path::PathBuf;

use crate::{MmioMap, ProjectSpec, Result};

pub(crate) fn load(paths: &[PathBuf], project: Option<&ProjectSpec>) -> Result<MmioMap> {
    let mut catalog = MmioMap::load_all(paths)?;
    if let Some(paths) = project.and_then(|project| project.registers.as_ref())
        && paths.model.is_file()
        && crate::registers::RegisterModel::is_model_file(&paths.model)?
    {
        let model = crate::registers::RegisterModel::load(&paths.model)?;
        let (model_svd, _) = model.render_svd()?;
        catalog.merge(MmioMap::parse(&model_svd)?)?;
    }
    Ok(catalog)
}
