//! Bind register admission to the exact contracts loaded by the project.
use super::*;
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, path::Path};

pub(super) fn enter(
    project: &ProjectSpec,
    manifest: &Path,
    mechanisms: &[String],
    svd: &MmioMap,
) -> Result<Option<open_radio_vendor_execution_model::admission::Scope>> {
    if mechanisms.is_empty() {
        return Ok(None);
    }
    let registry_path = project
        .verification
        .as_ref()
        .and_then(|w| w.model_inputs.as_ref())
        .ok_or_else(|| {
            crate::Error::invalid(
                "project verification requires model-inputs for execution admission",
            )
        })?;
    let registry: serde_json::Value = serde_json::from_slice(&std::fs::read(registry_path)?)?;
    let root = manifest
        .ancestors()
        .filter(|p| p.join("Cargo.toml").is_file())
        .last()
        .ok_or_else(|| crate::Error::invalid("model admission requires a repository root"))?
        .canonicalize()?;
    let declarations = registry["mechanisms"]
        .as_object()
        .ok_or_else(|| crate::Error::invalid("model mechanisms missing"))?;
    for name in mechanisms {
        if !declarations.contains_key(name) {
            return Err(crate::Error::invalid(format!(
                "unknown model mechanism {name}"
            )));
        }
    }
    let mut registers = BTreeMap::<String, BTreeSet<String>>::new();
    for (name, declaration) in declarations {
        for contract in declaration["contracts"].as_array().into_iter().flatten() {
            let relative = contract
                .as_str()
                .ok_or_else(|| crate::Error::invalid("invalid model contract"))?;
            let path = root.join(relative).canonicalize()?;
            let Some(loaded_hash) = project.loaded_model_inputs.get(&path) else {
                continue;
            };
            let bytes = std::fs::read(&path)?;
            if &format!("{:x}", Sha256::digest(&bytes)) != loaded_hash {
                return Err(crate::Error::invalid(format!(
                    "model contract changed after loading: {relative}"
                )));
            }
            let text =
                std::str::from_utf8(&bytes).map_err(|e| crate::Error::invalid(e.to_string()))?;
            let names: Vec<String> = if path.extension().is_some_and(|e| e == "svd") {
                MmioMap::parse(text)?
                    .registers
                    .into_iter()
                    .map(|r| r.name)
                    .collect()
            } else {
                let fragment: serde_json::Value = toml_edit::de::from_str(text)
                    .map_err(|e| crate::Error::invalid(e.to_string()))?;
                let prefixes = fragment["peripherals"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p["name"].as_str())
                    .map(|p| format!("{p}."))
                    .collect::<Vec<_>>();
                svd.registers
                    .iter()
                    .filter(|r| prefixes.iter().any(|p| r.name.starts_with(p)))
                    .map(|r| r.name.clone())
                    .collect()
            };
            for register in names {
                registers.entry(register).or_default().insert(name.clone());
            }
        }
    }
    let scope = open_radio_vendor_execution_model::admission::Scope::enter(mechanisms, registers)
        .map_err(|e| crate::Error::invalid(e.to_string()))?;
    open_radio_vendor_execution_model::admission::mechanism("target-abi");
    scope
        .check()
        .map_err(|e| crate::Error::invalid(e.to_string()))?;
    Ok(Some(scope))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loaded_register_contracts_are_checked_at_lookup_without_requiring_foreign_mechanisms() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest = root.join("verification/vendor/projects/esp32s31/vendor-project.toml");
        let mut project = ProjectSpec::load(&manifest).unwrap();
        let (map, inputs) =
            crate::register_catalog::load_with_inputs(&project.svd_paths, Some(&project)).unwrap();
        project.loaded_model_inputs.extend(inputs);
        let wifi = map
            .registers
            .iter()
            .find(|r| r.name.starts_with("WIFI_MAC_TX_COMMON."))
            .unwrap();
        for allowed in [false, true] {
            let mechanisms = if allowed {
                vec!["target-abi".into(), "wifi-registers".into()]
            } else {
                vec!["target-abi".into()]
            };
            let scope = enter(&project, &manifest, &mechanisms, &map)
                .unwrap()
                .unwrap();
            assert!(map.register(wifi.address).is_some());
            assert_eq!(scope.check().is_ok(), allowed);
        }
        let registry: serde_json::Value = serde_json::from_slice(
            &std::fs::read(
                project
                    .verification
                    .as_ref()
                    .unwrap()
                    .model_inputs
                    .as_ref()
                    .unwrap(),
            )
            .unwrap(),
        )
        .unwrap();
        let mechanisms = registry["mechanisms"]
            .as_object()
            .unwrap()
            .keys()
            .cloned()
            .collect::<Vec<_>>();
        let scope = enter(&project, &manifest, &mechanisms, &map)
            .unwrap()
            .unwrap();
        for register in &map.registers {
            map.register(register.address);
        }
        scope.check().unwrap();
    }
}
