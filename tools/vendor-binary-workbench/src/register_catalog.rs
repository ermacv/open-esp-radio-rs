//! Reusable composition of imported SVD and reviewed register-model data.

use std::path::PathBuf;

use crate::{MmioMap, ProjectSpec, Register, RegisterCatalog, Result};

pub(crate) fn load(paths: &[PathBuf], project: Option<&ProjectSpec>) -> Result<MmioMap> {
    let mut catalog = MmioMap::load_all(paths)?;
    if let Some(paths) = project.and_then(|project| project.registers.as_ref())
        && paths.model.is_file()
        && crate::registers::RegisterModel::is_model_file(&paths.model)?
    {
        let model = crate::registers::RegisterModel::load(&paths.model)?;
        catalog.merge(model_catalog(&model)?)?;
    }
    Ok(catalog)
}

fn model_catalog(model: &crate::registers::RegisterModel) -> Result<MmioMap> {
    let registers = model
        .register_identities()?
        .into_iter()
        .map(|((address, _width), name)| {
            Ok(Register {
                address: u32::try_from(address).map_err(|_| crate::WorkbenchError::invalid(
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

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    #[test]
    fn direct_model_adapter_matches_the_release_svd_catalog() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .unwrap();
        let model = crate::registers::RegisterModel::load(
            &root.join("verification/vendor/targets/esp32s31/registers/device.toml"),
        )
        .unwrap();
        let direct = model_catalog(&model).unwrap();
        let (svd, _) = model.render_svd().unwrap();
        let encoded = MmioMap::parse(&svd).unwrap();

        assert_eq!(direct.registers.len(), encoded.registers.len());
        assert!(
            direct
                .registers
                .iter()
                .zip(&encoded.registers)
                .all(|(direct, encoded)| direct.address == encoded.address)
        );
        // The typed model preserves an array's reviewed descending dimIndex;
        // the old XML collector numbered every expanded element from zero.
        assert_eq!(
            direct.register(0x2010_4178).unwrap().name,
            "WIFI_MAC_RX_DMA.RX_BLOCK_ACK_ENTRY7_CONTROL"
        );
    }
}
