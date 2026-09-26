//! Independent validation of a scenario seal inside a still-live campaign.

use super::*;

#[derive(Deserialize)]
struct Seal {
    schema: u16,
    manifest: RunManifest,
    suite: SuiteResult,
    files: Vec<IntegrityFile>,
}

pub(super) fn load(
    run: &Path,
    parent: &RunManifest,
) -> Result<Option<Vec<(RunManifest, SuiteResult)>>> {
    let directory = run.join("attempts");
    if !directory.try_exists()? {
        return Ok(None);
    }
    if !fs::symlink_metadata(&directory)?.file_type().is_dir() {
        return Err("HIL attempts must be a regular directory".into());
    }
    let mut entries = fs::read_dir(&directory)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(fs::DirEntry::file_name);
    let mut output = Vec::new();
    for entry in entries {
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_str().ok_or("attempt seal name is not UTF-8")?;
        // Atomic publication may leave an uncommitted temporary after SIGKILL.
        if name.starts_with('.') && name.contains(".tmp-") {
            continue;
        }
        let Some(id) = name.strip_suffix(".json").filter(|id| valid_id(id)) else {
            return Err("invalid HIL attempt seal name".into());
        };
        if !entry.file_type()?.is_file() {
            return Err("HIL attempt seal must be a regular file".into());
        }
        let seal: Seal = read_json(&path)?;
        let manifest = &seal.manifest;
        if seal.schema != 1
            || manifest.schema != HIL_RUN_SCHEMA
            || manifest.run_id != parent.run_id
            || manifest.target != parent.target
            || manifest.state != RunState::Completed
            || manifest.firmware.len() > 1
            || seal.suite.scenarios.len() != 1
            || seal.suite.scenarios[0].scenario != id
        {
            return Err("HIL attempt identity or completion boundary is inconsistent".into());
        }
        validate_suite(&seal.suite, manifest, run)?;
        let scenario_root = PathBuf::from("scenarios").join(id);
        let document: serde_json::Value =
            read_json(&run.join(&scenario_root).join("scenario.json"))?;
        if document.get("id").and_then(serde_json::Value::as_str) != Some(id) {
            return Err("HIL attempt procedure identity does not match its result".into());
        }
        let result: serde_json::Value = read_json(&run.join(&scenario_root).join("result.json"))?;
        // Only a Wi-Fi procedure names its image; other families imply it.
        // The sealed result carries the image either way and must agree
        // with an explicit procedure image and with the recorded firmware.
        let image = result
            .get("image")
            .and_then(serde_json::Value::as_str)
            .filter(|image| valid_id(image))
            .ok_or("HIL attempt has no valid image class")?;
        let raw: serde_json::Value = read_json(&path)?;
        if result != raw["suite"]["scenarios"][0]
            || document
                .pointer("/wifi/image")
                .is_some_and(|declared| declared.as_str() != Some(image))
            || document.get("repetitions") != result.get("required_repetitions")
            || raw["manifest"]["firmware"]
                .as_array()
                .is_none_or(|artifacts| artifacts.iter().any(|a| a["image"] != image))
        {
            return Err("HIL attempt subject, procedure or result is inconsistent".into());
        }
        let mut expected = Vec::new();
        let mut roots = vec![scenario_root, PathBuf::from("source")];
        if !manifest.firmware.is_empty() {
            let firmware = PathBuf::from("firmware").join(image);
            if !run.join(&firmware).try_exists()? {
                return Err("HIL attempt firmware material is missing".into());
            }
            roots.push(firmware);
        }
        for relative in roots {
            let path = run.join(&relative);
            if path.try_exists()? {
                require_regular_components(run, &relative, true)?;
                for (path, size) in collect_inventory(&path, false)? {
                    expected.push((relative.join(path), size));
                }
            }
        }
        for relative in ["plan.json", "lab-provenance.json"] {
            let path = run.join(relative);
            if path.try_exists()? {
                require_regular_components(run, Path::new(relative), false)?;
                expected.push((relative.into(), fs::metadata(path)?.len()));
            }
        }
        expected.sort();
        let mut files = seal.files;
        files.sort();
        let inventory = files
            .iter()
            .map(|f| (f.path.clone(), f.size_bytes))
            .collect::<Vec<_>>();
        if expected != inventory {
            return Err("HIL attempt does not match its complete material inventory".into());
        }
        for file in &files {
            require_regular_components(run, &file.path, false)?;
            if !valid_sha256(&file.sha256) || sha256_file(&run.join(&file.path))? != file.sha256 {
                return Err("HIL attempt material digest mismatch".into());
            }
        }
        for artifact in raw["manifest"]["firmware"].as_array().unwrap() {
            for (path_key, size_key, hash_key, required) in [
                (
                    "application_path",
                    "application_size_bytes",
                    "application_sha256",
                    true,
                ),
                (
                    "runtime_elf_path",
                    "runtime_elf_size_bytes",
                    "runtime_elf_sha256",
                    false,
                ),
                (
                    "runtime_bin_path",
                    "runtime_bin_size_bytes",
                    "runtime_bin_sha256",
                    false,
                ),
                (
                    "bootstrap_elf_path",
                    "bootstrap_elf_size_bytes",
                    "bootstrap_elf_sha256",
                    false,
                ),
            ] {
                if !required && artifact[path_key].is_null() {
                    continue;
                }
                let path = artifact[path_key]
                    .as_str()
                    .ok_or("HIL attempt firmware subject has no path")?;
                if !files.iter().any(|f| {
                    f.path == Path::new(path)
                        && Some(f.size_bytes) == artifact[size_key].as_u64()
                        && Some(f.sha256.as_str()) == artifact[hash_key].as_str()
                }) {
                    return Err(
                        "HIL attempt firmware identity does not match its sealed bytes".into(),
                    );
                }
            }
        }
        // Provenance cannot introduce unsealed external inputs. The existing
        // current-source policy performs the semantic build-binding check.
        for artifact in &manifest.firmware {
            if let Some(path) = &artifact.build_provenance_path
                && !files.iter().any(|f| &f.path == path)
            {
                return Err("HIL attempt build provenance is outside its sealed material".into());
            }
        }
        output.push((seal.manifest, seal.suite));
    }
    Ok(Some(output))
}

#[cfg(test)]
mod tests;

fn require_regular_components(root: &Path, relative: &Path, directory: bool) -> Result<()> {
    if !safe_relative(relative) {
        return Err("HIL attempt path escapes its run".into());
    }
    let mut path = root.to_owned();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        path.push(component);
        let metadata = fs::symlink_metadata(&path)?;
        let valid = if components.peek().is_some() || directory {
            metadata.file_type().is_dir()
        } else {
            metadata.file_type().is_file()
        };
        if !valid {
            return Err("HIL attempt material contains a symlink or special file".into());
        }
    }
    Ok(())
}
