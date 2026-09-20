//! Reviewed package scope over real resolved lockfiles and captured manifests.
//! Optional dependencies remain conservative; dev-only edges are excluded for
//! local owners, while build dependencies and target-specific edges are retained.
use super::*;
use serde_json::{Value, json};
use std::io::Read as _;

pub(super) fn archive_file(archive: &Path, path: &Path) -> Result<Vec<u8>> {
    for entry in tar::Archive::new(fs::File::open(archive)?).entries()? {
        let mut entry = entry?;
        if entry.path()?.as_ref() == path {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            return Ok(bytes);
        }
    }
    Err(format!("snapshot has no input {}", path.display()).into())
}
fn parse(bytes: &[u8]) -> Result<Value> {
    Ok(toml_edit::de::from_str(std::str::from_utf8(bytes)?)?)
}

pub(super) fn archived(
    run: &Path,
    sources: &[crate::hil::snapshot::Source],
    roots: &[String],
    lock: &[u8],
) -> Result<Value> {
    let directory = run.join("source/snapshot");
    let manifest = crate::hil::snapshot::verified(&directory, sources)?
        .ok_or("package scope requires verified captured sources")?;
    let paths = manifest
        .sources
        .iter()
        .flat_map(|source| {
            source
                .files
                .iter()
                .filter(|f| f.path.file_name().is_some_and(|n| n == "Cargo.toml"))
                .map(move |f| PathBuf::from(&source.name).join(&f.path))
        })
        .collect::<BTreeSet<_>>();
    let mut manifests = BTreeMap::new();
    for entry in tar::Archive::new(fs::File::open(directory.join("sources.tar"))?).entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        if paths.contains(&path) {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes)?;
            let path = path
                .strip_prefix("repository")
                .map(Path::to_owned)
                .unwrap_or_else(|_| PathBuf::from(".external").join(path));
            manifests.insert(path, parse(&bytes)?);
        }
    }
    let pins = archive_file(
        &directory.join("sources.tar"),
        Path::new("repository/Cargo.lock"),
    )?;
    Ok(
        json!({"effective":projection(&parse(lock)?, &manifests, roots)?,"pins":projection(&parse(&pins)?, &manifests, roots)?}),
    )
}

pub(super) fn current(
    root: &Path,
    roots: &[String],
    owners: &[PathBuf],
    composition: &Value,
) -> Result<bool> {
    let expected = &composition["locks"]["embedded-lock"]["pins"];
    let Some(selected) = expected["manifests"].as_object() else {
        return Ok(false);
    };
    let mut manifests = BTreeMap::new();
    for path in selected
        .keys()
        .map(PathBuf::from)
        .chain([PathBuf::from("Cargo.toml")])
    {
        regular(root, &path)?;
        manifests.insert(path.clone(), parse(&fs::read(root.join(path))?)?);
    }
    // A reviewer may narrow the dependency closure, but cannot omit a mapped
    // implementation package. Non-package contracts keep their byte bindings.
    for owner in owners {
        let mut directory = owner.parent();
        while let Some(path) = directory {
            let manifest = path.join("Cargo.toml");
            if root.join(&manifest).is_file() {
                let doc = parse(&fs::read(root.join(&manifest))?)?;
                if let Some(name) = doc["package"]["name"].as_str() {
                    if !roots.iter().any(|r| r == name) {
                        return Ok(false);
                    }
                    break;
                }
            }
            directory = path.parent();
        }
    }
    Ok(projection(
        &parse(&fs::read(root.join("Cargo.lock"))?)?,
        &manifests,
        roots,
    )? == *expected)
}

fn projection(
    lock: &Value,
    manifests: &BTreeMap<PathBuf, Value>,
    roots: &[String],
) -> Result<Value> {
    let workspace = manifests
        .get(Path::new("Cargo.toml"))
        .ok_or("captured workspace manifest is missing")?;
    let packages = lock["package"]
        .as_array()
        .ok_or("lockfile has no package graph")?;
    let mut local: BTreeMap<&str, Vec<_>> = BTreeMap::new();
    for (path, manifest) in manifests {
        if let Some(name) = manifest["package"]["name"].as_str() {
            local.entry(name).or_default().push((path, manifest));
        }
    }
    let mut pending = Vec::new();
    for root in roots {
        let candidates = packages
            .iter()
            .enumerate()
            .filter(|(_, p)| p["name"] == *root && p.get("source").is_none())
            .map(|(i, _)| i)
            .collect::<Vec<_>>();
        let [index] = candidates.as_slice() else {
            return Err(format!(
                "dependency root {root} is absent or ambiguous in the effective lockfile"
            )
            .into());
        };
        pending.push(*index);
    }
    let mut seen = BTreeSet::new();
    let mut bound_manifests = BTreeMap::new();
    let mut selected = BTreeMap::new();
    let mut dependency_names = BTreeSet::new();
    while let Some(index) = pending.pop() {
        if !seen.insert(index) {
            continue;
        }
        let package = &packages[index];
        let name = package["name"]
            .as_str()
            .ok_or("locked package has no name")?;
        dependency_names.insert(name.to_owned());
        let allowed = if package.get("source").is_none() {
            let candidates = local
                .get(name)
                .ok_or_else(|| format!("local dependency {name} has no captured manifest"))?;
            let [(path, manifest)] = candidates.as_slice() else {
                return Err(
                    format!("local dependency {name} has ambiguous captured manifests").into(),
                );
            };
            let mut manifest = (*manifest).clone();
            remove_dev_inputs(&mut manifest);
            let allowed = dependency_names_in(&manifest, workspace)?;
            bound_manifests.insert((*path).clone(), manifest);
            Some(allowed)
        } else {
            None
        };
        let mut dependencies = Vec::new();
        for dependency in package
            .get("dependencies")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let value = dependency.as_str().ok_or("invalid locked dependency")?;
            let parts = value.splitn(3, ' ').collect::<Vec<_>>();
            if allowed
                .as_ref()
                .is_some_and(|names| !names.contains(parts[0]))
            {
                continue;
            }
            let candidates = packages
                .iter()
                .enumerate()
                .filter(|(_, p)| {
                    p["name"] == parts[0]
                        && parts.get(1).is_none_or(|v| p["version"] == *v)
                        && parts.get(2).is_none_or(|v| {
                            p["source"].as_str() == Some(v.trim_matches(['(', ')']))
                        })
                })
                .map(|(i, _)| i)
                .collect::<Vec<_>>();
            let [index] = candidates.as_slice() else {
                return Err(
                    format!("locked dependency {value} cannot be resolved uniquely").into(),
                );
            };
            pending.push(*index);
            dependencies.push(value.to_owned());
        }
        dependencies.sort();
        let mut bound = package.clone();
        bound["dependencies"] = json!(dependencies);
        selected.insert(
            format!("{}@{}:{}", name, package["version"], package["source"]),
            bound,
        );
    }
    let mut workspace = workspace.clone();
    if let Some(table) = workspace
        .get_mut("workspace")
        .and_then(Value::as_object_mut)
    {
        table.remove("members");
        table.remove("default-members");
        table.remove("exclude");
        if let Some(deps) = table.get_mut("dependencies").and_then(Value::as_object_mut) {
            deps.retain(|name, value| {
                dependency_names
                    .contains(value.get("package").and_then(Value::as_str).unwrap_or(name))
            });
        }
    }
    remove_dev_inputs(&mut workspace);
    Ok(json!({"packages":selected,"workspace":workspace,"manifests":bound_manifests}))
}
fn remove_dev_inputs(manifest: &mut Value) {
    if let Some(object) = manifest.as_object_mut() {
        for key in ["dev-dependencies", "test", "bench", "example"] {
            object.remove(key);
        }
        if let Some(targets) = object.get_mut("target").and_then(Value::as_object_mut) {
            for target in targets.values_mut() {
                if let Some(target) = target.as_object_mut() {
                    target.remove("dev-dependencies");
                }
            }
        }
    }
}
fn dependency_names_in(manifest: &Value, workspace: &Value) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    let scopes = std::iter::once(manifest).chain(
        manifest
            .get("target")
            .and_then(Value::as_object)
            .into_iter()
            .flat_map(|t| t.values()),
    );
    for scope in scopes {
        for kind in ["dependencies", "build-dependencies"] {
            for (alias, dependency) in scope
                .get(kind)
                .and_then(Value::as_object)
                .into_iter()
                .flatten()
            {
                let dependency =
                    if dependency.get("workspace").and_then(Value::as_bool) == Some(true) {
                        workspace
                            .pointer(&format!("/workspace/dependencies/{alias}"))
                            .ok_or("inherited dependency is absent")?
                    } else {
                        dependency
                    };
                names.insert(
                    dependency
                        .get("package")
                        .and_then(Value::as_str)
                        .unwrap_or(alias)
                        .to_owned(),
                );
            }
        }
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> (Value, BTreeMap<PathBuf, Value>, Vec<String>) {
        let lock = json!({"package":[
            {"name":"wifi","version":"1","dependencies":["wire","test-only"]},
            {"name":"ble","version":"1","dependencies":["ble-dep"]},
            {"name":"wire","version":"1","source":"registry+test","checksum":"a"},
            {"name":"ble-dep","version":"1","source":"registry+test","checksum":"b"},
            {"name":"test-only","version":"1","source":"registry+test","checksum":"c"}
        ]});
        let manifests = BTreeMap::from([
            (
                PathBuf::from("Cargo.toml"),
                json!({"workspace":{"members":["wifi","ble"],"dependencies":{"wire":"1","ble-dep":"1"}},"profile":{"release":{"opt-level":3}}}),
            ),
            (
                PathBuf::from("wifi/Cargo.toml"),
                json!({"package":{"name":"wifi","version":"1"},"dependencies":{"wire":{"workspace":true}},"dev-dependencies":{"test-only":"1"}}),
            ),
            (
                PathBuf::from("ble/Cargo.toml"),
                json!({"package":{"name":"ble","version":"1"},"dependencies":{"ble-dep":"1"}}),
            ),
        ]);
        (lock, manifests, vec!["wifi".into()])
    }
    #[test]
    fn unrelated_packages_and_test_only_dependencies_do_not_change_runtime_closure() {
        let (mut lock, mut manifests, roots) = fixture();
        let original = projection(&lock, &manifests, &roots).unwrap();
        lock["package"][1]["version"] = json!("2");
        lock["package"][3]["checksum"] = json!("changed BLE");
        lock["package"][4]["checksum"] = json!("changed test");
        manifests.get_mut(Path::new("Cargo.toml")).unwrap()["workspace"]["dependencies"]["ble-dep"] =
            json!("2");
        manifests.get_mut(Path::new("wifi/Cargo.toml")).unwrap()["dev-dependencies"]["test-only"] =
            json!("2");
        assert_eq!(projection(&lock, &manifests, &roots).unwrap(), original);
        lock["package"][2]["checksum"] = json!("changed runtime");
        assert_ne!(projection(&lock, &manifests, &roots).unwrap(), original);
    }
    #[test]
    fn build_target_and_profile_changes_remain_bound_and_ambiguous_roots_fail() {
        let (lock, mut manifests, roots) = fixture();
        let original = projection(&lock, &manifests, &roots).unwrap();
        manifests.get_mut(Path::new("wifi/Cargo.toml")).unwrap()["target"] =
            json!({"cfg(test-target)":{"build-dependencies":{"test-only":"1"}}});
        let changed = projection(&lock, &manifests, &roots).unwrap();
        assert_ne!(changed, original);
        assert!(
            changed["packages"]
                .as_object()
                .unwrap()
                .keys()
                .any(|key| key.starts_with("test-only@"))
        );
        manifests.get_mut(Path::new("Cargo.toml")).unwrap()["profile"]["release"]["opt-level"] =
            json!(1);
        assert_ne!(projection(&lock, &manifests, &roots).unwrap(), changed);
        manifests.insert(
            "other/Cargo.toml".into(),
            manifests[Path::new("wifi/Cargo.toml")].clone(),
        );
        assert!(projection(&lock, &manifests, &roots).is_err());
        assert!(projection(&lock, &manifests, &["missing".into()]).is_err());
    }
}
