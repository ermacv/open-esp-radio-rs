//! Explicit suite dependencies on target/ABI, model mechanisms and their implementation.
use super::{Result, contained, read, string};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

pub fn files(root: &Path, paths: &Value) -> Result<BTreeMap<String, String>> {
    fn visit(root: &Path, path: &Path, output: &mut BTreeMap<String, String>) -> Result<()> {
        let path = path.canonicalize()?;
        if !path.starts_with(root.canonicalize()?) {
            return Err("model input escapes repository".into());
        }
        if path.is_dir() {
            for entry in fs::read_dir(path)? {
                let entry = entry?;
                let name = entry.file_name();
                if matches!(
                    name.to_str(),
                    Some("tests" | "tests.rs" | "test_support.rs")
                ) {
                    continue;
                }
                if entry.path().is_dir()
                    || entry.path().extension().is_some_and(|e| e == "rs")
                    || name == "Cargo.toml"
                {
                    visit(root, &entry.path(), output)?;
                }
            }
        } else {
            output.insert(
                path.strip_prefix(root.canonicalize()?)?
                    .to_string_lossy()
                    .into_owned(),
                format!("{:x}", Sha256::digest(fs::read(path)?)),
            );
        }
        Ok(())
    }
    let mut output = BTreeMap::new();
    for path in paths.as_array().ok_or("model paths must be an array")? {
        visit(
            root,
            &root.join(path.as_str().ok_or("invalid model input path")?),
            &mut output,
        )?;
    }
    Ok(output)
}

pub fn current(
    root: &Path,
    project_path: &Path,
    project: &Value,
    addon_path: &Path,
    addon: &Value,
    suite: &Value,
) -> Result<Value> {
    let registry: Value = serde_json::from_slice(&fs::read(contained(
        root,
        &addon_path
            .parent()
            .unwrap()
            .join(string(addon, "model-inputs")?),
    )?)?)?;
    if registry["schema"] != 1 {
        return Err("unsupported model input registry".into());
    }
    let mechanisms = suite["model-mechanisms"]
        .as_array()
        .filter(|a| !a.is_empty())
        .ok_or("suite model mechanisms missing")?;
    let mut implementation = BTreeMap::new();
    let mut contracts = BTreeMap::new();
    for mechanism in mechanisms {
        let key = mechanism.as_str().ok_or("invalid model mechanism")?;
        let specification = registry["mechanisms"]
            .get(key)
            .ok_or("unknown suite model mechanism")?;
        implementation.extend(files(root, &specification["implementation"])?);
        contracts.extend(files(root, &specification["contracts"])?);
    }
    let base = project_path.parent().unwrap();
    let target = read(&contained(
        root,
        &base.join(string(project, "target-spec")?),
    )?)?;
    let chip_path = contained(root, &base.join(string(project, "chip-pack")?))?;
    let chip = read(&chip_path)?;
    let chip_base = chip_path.parent().unwrap();
    let mut active = std::collections::BTreeSet::new();
    for key in ["memory-map"] {
        if let Some(relative) = chip[key].as_str() {
            active.insert(contained(root, &chip_base.join(relative))?);
        }
    }
    for relative in chip["svd"].as_array().into_iter().flatten() {
        active.insert(contained(
            root,
            &chip_base.join(relative.as_str().ok_or("invalid chip SVD path")?),
        )?);
    }
    if let Some(relative) = chip["register-model"].as_str() {
        let model_path = contained(root, &chip_base.join(relative))?;
        let model = read(&model_path)?;
        for relative in model["fragments"]
            .as_array()
            .ok_or("model fragments missing")?
        {
            active.insert(contained(
                root,
                &model_path
                    .parent()
                    .unwrap()
                    .join(relative.as_str().ok_or("invalid fragment path")?),
            )?);
        }
    }
    for relative in project["reviewed-knowledge"]["packs"]
        .as_array()
        .into_iter()
        .flatten()
    {
        active.insert(contained(
            root,
            &base.join(relative.as_str().ok_or("invalid reviewed knowledge path")?),
        )?);
    }
    if contracts
        .keys()
        .any(|path| !active.contains(&root.join(path)))
    {
        return Err("suite model contract is not selected by the current chip/project".into());
    }
    let mut target = target.as_object().ok_or("invalid target contract")?.clone();
    target.remove("schema");
    target.insert(
        "knowledge_provider".into(),
        project
            .get("analysis-provider")
            .unwrap_or(&chip["knowledge-provider"])
            .clone(),
    );
    // Use the report's field names; values describe the actual target supplied to execution.
    for (old, new) in [
        ("calling-convention", "calling_convention"),
        ("pointer-width", "pointer_width"),
        ("rust-target", "rust_target"),
    ] {
        let value = target.remove(old).ok_or("incomplete target contract")?;
        target.insert(new.into(), value);
    }
    let mut ecosystems = std::collections::BTreeSet::new();
    for path in project["ecosystem-packs"].as_array().into_iter().flatten() {
        let pack = read(&contained(
            root,
            &base.join(path.as_str().ok_or("invalid ecosystem path")?),
        )?)?;
        for ecosystem in pack["applicability"]["ecosystems"]
            .as_array()
            .into_iter()
            .flatten()
        {
            ecosystems.insert(ecosystem.as_str().ok_or("invalid ecosystem")?.to_owned());
        }
    }
    let dimension = |value: &Value| {
        let mut values = value.as_array().cloned().unwrap_or_default();
        values.sort_by_key(|v| v.as_str().unwrap_or_default().to_owned());
        values
    };
    let context = json!({"ecosystems":ecosystems,"chips":dimension(&chip["applicability"]["chips"]),"chip_revisions":dimension(&chip["applicability"]["chip-revisions"]),"artifact_lineages":dimension(&project["applicability"]["artifact-lineages"])});
    Ok(
        json!({"schema":1,"mechanisms":mechanisms,"target":target,"implementation":implementation,"contracts":contracts,
        "provider": {"context":context,"chip":chip["id"],"base":chip["knowledge-provider"],"overlay":project["analysis-provider"]}}),
    )
}

/// The producer supplies complete loaded provenance; only explicit suite inputs apply.
#[allow(dead_code)] // Only the producer needs the full execution record.
pub fn matches(current: &Value, executed: Option<&Value>) -> bool {
    let Some(executed) = executed else {
        return false;
    };
    current["schema"] == executed["schema"]
        && current["mechanisms"] == executed["mechanisms"]
        && current["target"] == executed["target"]
        && current["provider"] == executed["provider"]
        && ["implementation", "contracts"].iter().all(|key| {
            current[*key].as_object().is_some_and(|inputs| {
                inputs
                    .iter()
                    .all(|(path, hash)| executed[*key].get(path) == Some(hash))
            })
        })
}

#[cfg(test)]
pub fn fixture(root: &Path) {
    fs::write(
        root.join("model-inputs.json"),
        r#"{"schema":1,"mechanisms":{"abi":{"implementation":["model.rs"],"contracts":[]}}}"#,
    )
    .unwrap();
    fs::write(root.join("model.rs"), "model A").unwrap();
    fs::write(root.join("target.toml"), "schema = 3\nid = 'fixture'\narchitecture = 'riscv32'\ncalling-convention = 'riscv-ilp32'\nendianness = 'little'\npointer-width = 32\nrust-target = 'riscv32imac-unknown-none-elf'\n").unwrap();
    fs::write(root.join("chip.toml"), "id = 'chip'\n").unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn a_suite_uses_its_loaded_models_without_rebinding_to_new_or_foreign_models() {
        let current = json!({"schema":1,"mechanisms":["wifi"],"target":{"id":"target"},"provider":{"base":"base"},"implementation":{"wifi.rs":"A"},"contracts":{"wifi.toml":"C"}});
        let mut executed = current.clone();
        executed["implementation"]["ble.rs"] = json!("B");
        assert!(matches(&current, Some(&executed)));
        executed["implementation"]["ble.rs"] = json!("new BLE model");
        assert!(matches(&current, Some(&executed)));
        executed["implementation"]["wifi.rs"] = json!("new Wi-Fi model");
        assert!(!matches(&current, Some(&executed)));
        assert!(!matches(&current, None));
    }
}
