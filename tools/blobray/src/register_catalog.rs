//! Reusable composition of imported SVD and reviewed register-model data.

use std::path::PathBuf;

use crate::{MmioMap, ProjectSpec, Register, RegisterCatalog, Result};

pub(crate) fn load(paths: &[PathBuf], project: Option<&ProjectSpec>) -> Result<MmioMap> {
    Ok(load_with_inputs(paths, project)?.0)
}

pub(crate) fn load_with_inputs(
    paths: &[PathBuf],
    project: Option<&ProjectSpec>,
) -> Result<(MmioMap, std::collections::BTreeMap<PathBuf, String>)> {
    use sha2::{Digest, Sha256};
    let mut inputs = std::collections::BTreeMap::new();
    let mut catalog = MmioMap::load_all(&[])?;
    for path in paths {
        let bytes = std::fs::read(path)?;
        let xml = std::str::from_utf8(&bytes)
            .map_err(|error| crate::Error::invalid(error.to_string()))?;
        let registers = open_esp_radio_register_model::svd_geometry(xml)?
            .into_iter()
            .map(|geometry| {
                Ok(Register {
                    address: u32::try_from(geometry.address).map_err(|_| {
                        crate::Error::invalid(
                            "SVD address exceeds the current 32-bit analysis backend",
                        )
                    })?,
                    name: geometry.name,
                })
            })
            .collect::<Result<Vec<_>>>()?;
        catalog.merge(MmioMap {
            registers,
            regions: Vec::new(),
        })?;
        inputs.insert(
            path.canonicalize()?,
            format!("{:x}", Sha256::digest(&bytes)),
        );
    }
    if let Some(paths) = project.and_then(|project| project.registers.as_ref())
        && paths.model.is_file()
        && crate::registers::RegisterModel::is_model_file(&paths.model)?
    {
        // Review loaders consume private copies too. The geometry loader retains
        // the exact manifest/fragment text it parsed, including relative imports.
        let mut copies = crate::verification::ExecutionInputs::new()?;
        let mut captured = paths.clone();
        for path in &mut captured.reviewed_knowledge {
            let original = path.canonicalize()?;
            copies.capture(path)?;
            inputs.insert(
                original,
                format!("{:x}", Sha256::digest(std::fs::read(path)?)),
            );
        }
        let model = crate::registers::load_effective_register_model(&captured)?;
        for (path, text) in model.loaded_inputs() {
            inputs.insert(
                path.clone(),
                format!("{:x}", Sha256::digest(text.as_bytes())),
            );
        }
        catalog.merge(model_catalog(&model)?)?;
    }
    Ok((catalog, inputs))
}

fn model_catalog(model: &crate::registers::RegisterModel) -> Result<MmioMap> {
    let registers = model
        .register_identities()?
        .into_iter()
        .map(|((address, _width), name)| {
            Ok(Register {
                address: u32::try_from(address).map_err(|_| crate::BlobrayError::invalid(
                    format!(
                        "register model identity {name} has address {address:#018x} outside the 32-bit target address space"
                    )
                ))?,
                name,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let mut catalog = RegisterCatalog::default();
    catalog.merge(RegisterCatalog { registers })?;
    Ok(MmioMap {
        registers: catalog.registers,
        regions: Vec::new(),
    })
}
