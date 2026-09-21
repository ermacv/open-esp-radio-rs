//! The executed host observer has its own build subject, separate from firmware.
use super::*;
use serde_json::Value;

fn prefixes(root: &Path, document: Option<&Value>) -> Result<Vec<PathBuf>> {
    let registry: Value = read_json(&root.join("hil/schema/observer-inputs.json"))?;
    if registry["schema"] != 1 {
        return Err("unsupported observer input registry".into());
    }
    let mut prefixes = registry["common"]
        .as_array()
        .ok_or("observer common inputs missing")?
        .clone();
    if let Some(kind) = document
        .and_then(|d| d.pointer("/workload/kind"))
        .and_then(Value::as_str)
    {
        prefixes.extend(
            registry["workloads"][kind]
                .as_array()
                .ok_or("unmapped workload observer")?
                .iter()
                .cloned(),
        );
    }
    prefixes
        .into_iter()
        .map(|prefix| {
            let path = PathBuf::from(prefix.as_str().ok_or("invalid observer input")?);
            if !safe_relative(&path) {
                return Err("observer input escapes repository".into());
            }
            Ok(path)
        })
        .collect()
}

fn selected(path: &Path, prefixes: &[PathBuf]) -> bool {
    prefixes.iter().any(|prefix| {
        path == prefix
            || (path.starts_with(prefix)
                && path.extension().is_some_and(|e| e == "rs")
                && !path.components().any(|c| c.as_os_str() == "tests")
                && !path
                    .file_name()
                    .is_some_and(|s| s == "tests.rs" || s == "test_support.rs"))
    })
}

fn inputs(root: &Path, prefixes: &[PathBuf]) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    for relative in prefixes {
        let path = root.join(relative);
        if fs::symlink_metadata(&path)?.file_type().is_dir() {
            for (child, _) in collect_inventory(&path, false)? {
                if child.extension().is_some_and(|e| e == "rs") {
                    let relative = relative.join(child);
                    if !selected(&relative, prefixes) {
                        continue;
                    }
                    files.insert(
                        relative.to_string_lossy().into_owned(),
                        sha256_file(&root.join(relative))?,
                    );
                }
            }
        } else {
            let identity = subject::file(root, relative)?.ok_or("observer input missing")?;
            files.insert(relative.to_string_lossy().into_owned(), identity.sha256);
        }
    }
    if files.is_empty() {
        return Err("observer input set is empty".into());
    }
    Ok(files)
}

pub(super) fn matches(
    root: &Path,
    observation: &ScenarioEvidence,
    proof: Option<&Value>,
) -> Result<bool> {
    let Some(proof) = proof.or_else(|| {
        observation
            .subject
            .as_ref()
            .and_then(|s| s.observer.as_ref())
    }) else {
        return Ok(false);
    };
    if proof["schema"] != 1
        || proof["build"]["schema"] != 1
        || !proof["executable_sha256"]
            .as_str()
            .is_some_and(valid_sha256)
        || proof["build_sha256"].as_str()
            != Some(&format!(
                "{:x}",
                Sha256::digest(serde_json::to_vec(&proof["build"])?)
            ))
    {
        return Ok(false);
    }
    let document = observation
        .run_directory
        .as_ref()
        .zip(
            observation
                .subject
                .as_ref()
                .and_then(|s| s.procedure.as_ref()),
        )
        .map(|(run, p)| read_json::<Value>(&run.join(&p.path)))
        .transpose()?;
    let prefixes = prefixes(root, document.as_ref())?;
    let expected = inputs(root, &prefixes)?;
    let Some(recorded) = proof["build"]["inputs"].as_object() else {
        return Ok(false);
    };
    let relevant = recorded
        .iter()
        .filter(|(path, _)| selected(Path::new(path), &prefixes))
        .map(|(path, hash)| (path.clone(), hash.as_str().unwrap_or_default().to_owned()))
        .collect::<BTreeMap<_, _>>();
    Ok(expected == relevant)
}

/// A legacy observation needs an explicit reviewer-owned execution/build binding.
/// Its supporting documents must be hashed evidence inputs in the same review.
pub(super) fn reviewed(
    root: &Path,
    observation: &ScenarioEvidence,
    scenario: &str,
    path: &Path,
    evidence: &BTreeMap<PathBuf, String>,
) -> Result<bool> {
    let Some(identity) = subject::file(root, path)? else {
        return Ok(false);
    };
    if evidence.get(path) != Some(&identity.sha256) {
        return Ok(false);
    }
    let document: Value = read_json(&root.join(path))?;
    if document["observation_id"].as_str() != observation.observation_id(scenario).as_deref() {
        return Ok(false);
    }
    let Some(support) = document["supporting_evidence"]
        .as_array()
        .filter(|s| !s.is_empty())
    else {
        return Ok(false);
    };
    for file in support {
        let Some(path) = file["path"].as_str() else {
            return Ok(false);
        };
        let Some(identity) = subject::file(root, Path::new(path))? else {
            return Ok(false);
        };
        if evidence.get(Path::new(path)) != Some(&identity.sha256)
            || Some(identity.sha256.as_str()) != file["sha256"].as_str()
        {
            return Ok(false);
        }
    }
    matches(root, observation, Some(&document["observer"]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_observers_are_mapped_without_binding_other_workloads_or_reports() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let catalog = ScenarioCatalog::load(&root, Path::new("hil/scenarios")).unwrap();
        let mut seen = BTreeSet::new();
        for document in catalog.definitions.values() {
            let kind = document
                .pointer("/workload/kind")
                .unwrap()
                .as_str()
                .unwrap();
            if seen.insert(kind) {
                let prefixes = prefixes(&root, Some(document)).unwrap();
                assert!(!inputs(&root, &prefixes).unwrap().is_empty());
                assert!(!selected(
                    Path::new("hil/host/runner/src/reporting/history.rs"),
                    &prefixes
                ));
                if kind == "udp" {
                    assert!(selected(
                        Path::new("hil/host/runner/src/workload/traffic/rx_traffic.rs"),
                        &prefixes
                    ));
                    assert!(!selected(
                        Path::new("hil/host/runner/src/workload/bluetooth/deadline.rs"),
                        &prefixes
                    ));
                }
            }
        }
    }
}
