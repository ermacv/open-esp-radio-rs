//! Package closure shared by build provenance and applicability evaluation.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn projection(
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
/// Resolve normal/build dependency names, including aliases and workspace inheritance.
pub fn dependency_names_in(manifest: &Value, workspace: &Value) -> Result<BTreeSet<String>> {
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
