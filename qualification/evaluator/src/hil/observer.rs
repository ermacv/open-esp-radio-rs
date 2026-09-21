//! The executed host observer has its own build subject, separate from firmware.
use super::*;
use serde_json::{Value, json};
#[path = "../../../../hil/schema/observer-build.rs"]
pub(super) mod build_inputs;

fn prefixes(root: &Path, document: Option<&Value>) -> Result<Vec<PathBuf>> {
    let registry: Value = read_json(&root.join("hil/schema/observer-inputs.json"))?;
    if registry["schema"] != 2 {
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
        let domain = registry["workload_domains"][kind]
            .as_str()
            .ok_or("observer workload domain missing")?;
        prefixes.extend(
            registry["domains"][domain]
                .as_array()
                .ok_or("observer domain inputs missing")?
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
                && (path
                    .extension()
                    .is_some_and(|e| e == "rs" || e == "uc" || e == "sh")
                    || path.file_name().is_some_and(|n| n == "Cargo.toml"))
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
                if child
                    .extension()
                    .is_some_and(|e| e == "rs" || e == "uc" || e == "sh")
                    || child.file_name().is_some_and(|n| n == "Cargo.toml")
                {
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
    compatible(root, observation, proof, None)
}

pub(super) fn compatible(
    root: &Path,
    observation: &ScenarioEvidence,
    proof: Option<&Value>,
    configuration_review: Option<&Value>,
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
        || proof["build"]["schema"] != 2
        || proof["build"]["resolved"]["compilation"] != "cargo-compiler-artifacts-v1"
        || proof["build"]["resolved"]["selected_profile"]
            .as_str()
            .is_none_or(str::is_empty)
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
    let registry: Value = read_json(&root.join("hil/schema/observer-inputs.json"))?;
    let kind = document
        .as_ref()
        .and_then(|d| d.pointer("/workload/kind"))
        .and_then(Value::as_str)
        .unwrap_or("");
    let dependencies = build_inputs::dependencies(&registry, kind)?;
    let Ok(current) = build_inputs::resolve_compiled(root) else {
        return Ok(false);
    };
    if build_inputs::validate_registry(&current, &registry).is_err() {
        return Ok(false);
    }
    let mut old_dependencies =
        build_inputs::projection(&proof["build"]["resolved"], &dependencies)?;
    let mut new_dependencies = build_inputs::projection(&current, &dependencies)?;
    let old_units = build_inputs::take_unit_profiles(&mut old_dependencies);
    let new_units = build_inputs::take_unit_profiles(&mut new_dependencies);
    for projection in [&mut old_dependencies, &mut new_dependencies] {
        projection["workspace"]
            .as_object_mut()
            .ok_or("workspace missing")?
            .remove("profile");
    }
    if old_dependencies != new_dependencies {
        return Ok(false);
    }
    let mut prefixes = prefixes(root, document.as_ref())?;
    for path in new_dependencies["manifests"]
        .as_object()
        .ok_or("observer manifests missing")?
        .keys()
    {
        if path == "hil/host/runner/Cargo.toml" {
            continue;
        }
        let directory = Path::new(path)
            .parent()
            .ok_or("dependency directory missing")?;
        for path in [
            directory.join("src"),
            directory.join("build.rs"),
            PathBuf::from(path),
        ] {
            if root.join(&path).exists() {
                prefixes.push(path);
            }
        }
    }
    // Used manifests have already been compared through the shared Cargo
    // projection. Their raw bytes remain provenance, but dev-only declarations
    // must not reintroduce an unrelated dependency through the file selector.
    let projected_manifest = |path: &str| new_dependencies["manifests"].get(path).is_some();
    let mut expected = inputs(root, &prefixes)?;
    expected.retain(|path, _| !projected_manifest(path));
    let Some(recorded) = proof["build"]["inputs"].as_object() else {
        return Ok(false);
    };
    let relevant = recorded
        .iter()
        .filter(|(path, _)| selected(Path::new(path), &prefixes) && !projected_manifest(path))
        .map(|(path, hash)| (path.clone(), hash.as_str().unwrap_or_default().to_owned()))
        .collect::<BTreeMap<_, _>>();
    if expected != relevant {
        return Ok(false);
    }
    let actual = json!({"units":old_units,"cargo":proof["build"]["resolved"]["cargo_config"],"compiler":proof["build"]["compiler"],"environment":proof["build"]["environment"],"profiles":profile_configuration(&proof["build"]["resolved"])});
    let mut required = current["configuration"].clone();
    required["units"] = new_units;
    required["cargo"] = current["cargo_config"].clone();
    required["profiles"] = profile_configuration(&current);
    if actual == required {
        return Ok(true);
    }
    let mut actual_environment = actual["environment"].clone();
    let mut required_environment = required["environment"].clone();
    for environment in [&mut actual_environment, &mut required_environment] {
        let Some(environment) = environment.as_object_mut() else {
            return Ok(false);
        };
        for field in ["PROFILE", "OPT_LEVEL", "DEBUG"] {
            environment.remove(field);
        }
    }
    if actual_environment != required_environment || actual["cargo"] != required["cargo"] {
        return Ok(false);
    }
    Ok(configuration_review.is_some_and(|review| {
        review["build_sha256"] == proof["build_sha256"]
            && review["required_sha256"]
                == json!(format!(
                    "{:x}",
                    Sha256::digest(serde_json::to_vec(&required).unwrap())
                ))
            && review["reason"]
                .as_str()
                .is_some_and(|s| !s.trim().is_empty())
    }))
}

/// A legacy observation needs an explicit reviewer-owned execution/build binding.
/// Its supporting documents must be hashed evidence inputs in the same review.
pub(super) fn reviewed(
    root: &Path,
    observation: &ScenarioEvidence,
    scenario: &str,
    path: &Path,
    evidence: &BTreeMap<PathBuf, String>,
    configuration_reviews: &[Value],
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
    if matches(root, observation, Some(&document["observer"]))? {
        return Ok(true);
    }
    for configuration in configuration_reviews {
        if compatible(
            root,
            observation,
            Some(&document["observer"]),
            Some(configuration),
        )? {
            return Ok(true);
        }
    }
    Ok(false)
}

#[cfg(test)]
pub(super) fn required_configuration(root: &Path, registry: &Value) -> Result<Value> {
    let output = std::process::Command::new("rustc")
        .current_dir(root)
        .arg("-vV")
        .output()?;
    if !output.status.success() {
        return Err("cannot identify required observer compiler".into());
    }
    let compiler = String::from_utf8(output.stdout)?;
    let target = compiler
        .lines()
        .find_map(|line| line.strip_prefix("host: "))
        .ok_or("compiler host missing")?;
    let flags = std::env::var("CARGO_ENCODED_RUSTFLAGS").unwrap_or_else(|_| {
        std::env::var("RUSTFLAGS")
            .map(|flags| flags.split_whitespace().collect::<Vec<_>>().join("\u{1f}"))
            .unwrap_or_else(|_| {
                registry["build"]["rustflags"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join("\u{1f}")
            })
    });
    Ok(json!({"compiler":compiler,"environment":{
        "TARGET":target,"PROFILE":registry["build"]["profile"],"OPT_LEVEL":registry["build"]["opt_level"],"DEBUG":registry["build"]["debug"],"CARGO_ENCODED_RUSTFLAGS":flags
    }}))
}

fn profile_configuration(resolved: &Value) -> Value {
    let Some(mut name) = resolved["selected_profile"].as_str() else {
        return Value::Null;
    };
    let mut profiles = BTreeMap::new();
    loop {
        let profile = &resolved["manifests"]["Cargo.toml"]["profile"][name];
        if profiles.insert(name.to_owned(), profile.clone()).is_some() {
            return Value::Null;
        }
        let Some(parent) = profile["inherits"].as_str() else {
            break;
        };
        name = parent;
    }
    json!(profiles)
}

/// Host build compatibility is independent of firmware image placement policy.
pub(super) fn timing_sensitive(
    root: &Path,
    requirement: &HilRequirement,
    catalog: &ScenarioCatalog,
) -> Result<bool> {
    if catalog
        .checks
        .get(&requirement.scenario)
        .is_some_and(|checks| {
            checks.iter().any(|(name, check)| {
                (requirement.checks.is_empty() || requirement.checks.contains(name))
                    && check.image_sensitive()
            })
        })
    {
        return Ok(true);
    }
    if !requirement.checks.is_empty() {
        return Ok(false);
    }
    let Some(kind) = catalog
        .definitions
        .get(&requirement.scenario)
        .and_then(|d| d.pointer("/workload/kind"))
        .and_then(Value::as_str)
    else {
        return Ok(false);
    };
    let registry: Value = read_json(&root.join("hil/schema/observer-inputs.json"))?;
    registry["timing"][kind]
        .as_bool()
        .ok_or_else(|| "observer timing policy missing".into())
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
                    assert!(!selected(
                        Path::new("hil/host/runner/src/fixture/bluetooth/att.rs"),
                        &prefixes
                    ));
                    assert!(!selected(Path::new("Cargo.lock"), &prefixes));
                    assert!(selected(Path::new("hil/protocol/Cargo.toml"), &prefixes));
                    assert!(selected(Path::new("tools/process/Cargo.toml"), &prefixes));
                }
            }
        }
    }
}
