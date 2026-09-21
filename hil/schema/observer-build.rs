//! Host build inputs shared by the runner producer and independent evaluator.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};
#[path = "cargo-inputs.rs"]
pub mod cargo_inputs;
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[path = "observer-artifacts.rs"]
mod artifacts;

#[allow(dead_code)]
pub fn resolve_compiled(root: &Path) -> Result<Value> {
    let compilation = artifacts::compile(root).map_err(|e| e.to_string())?;
    let output = Command::new(&compilation.executable)
        .arg("--observer-build")
        .output()?;
    if !output.status.success() {
        return Err("cannot identify current compiled observer configuration".into());
    }
    let built: Value = serde_json::from_slice(&output.stdout)?;
    let mut resolved = resolve(
        root,
        built["environment"]["TARGET"]
            .as_str()
            .ok_or("compiled observer target missing")?,
    )?;
    artifacts::apply(&mut resolved, &compilation.artifacts).map_err(|e| e.to_string())?;
    resolved["selected_profile"] = json!(compilation.profile);
    resolved["configuration"] =
        json!({"compiler":built["compiler"],"environment":built["environment"]});
    Ok(resolved)
}

const RUNNER: &str = "open-esp-radio-hil-runner";

pub fn resolve(root: &Path, target: &str) -> Result<Value> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args([
            "tree",
            "--locked",
            "--offline",
            "-p",
            RUNNER,
            "--edges",
            "normal,build",
            "--prefix",
            "depth",
            "--no-dedupe",
            "--format",
            "{p}|{f}",
            "--target",
            target,
        ])
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "observer Cargo resolution failed: {}",
            String::from_utf8_lossy(&output.stderr)
        )
        .into());
    }
    let lock: Value = toml_edit::de::from_str(&fs::read_to_string(root.join("Cargo.lock"))?)?;
    let packages = lock["package"].as_array().ok_or("missing lock packages")?;
    let mut nodes = Vec::new();
    let mut manifests = BTreeMap::new();
    manifests.insert(
        PathBuf::from("Cargo.toml"),
        toml_edit::de::from_str::<Value>(&fs::read_to_string(root.join("Cargo.toml"))?)?,
    );
    for line in std::str::from_utf8(&output.stdout)?.lines() {
        let boundary = line
            .find(|c: char| !c.is_ascii_digit())
            .ok_or("invalid Cargo tree depth")?;
        let depth: usize = line[..boundary].parse()?;
        let (identity, features) = line[boundary..]
            .split_once('|')
            .ok_or("invalid Cargo tree node")?;
        let identity = identity.replace(" (proc-macro)", "");
        let mut words = identity.split_whitespace();
        let name = words.next().ok_or("missing package name")?;
        let version = words
            .next()
            .and_then(|v| v.strip_prefix('v'))
            .ok_or("missing package version")?;
        let candidates = packages
            .iter()
            .filter(|p| {
                p["name"] == name
                    && p["version"] == version
                    && match identity
                        .split_once(" (")
                        .map(|(_, s)| s.trim_end_matches(')'))
                    {
                        None => p["source"]
                            .as_str()
                            .is_some_and(|s| s.starts_with("registry+")),
                        Some(location) if location.starts_with('/') => p.get("source").is_none(),
                        Some(location) => p["source"].as_str().is_some_and(|s| {
                            s.strip_prefix("git+")
                                .is_some_and(|s| s.starts_with(location))
                        }),
                    }
            })
            .collect::<Vec<_>>();
        let [package] = candidates.as_slice() else {
            return Err(format!("ambiguous observer package {identity}").into());
        };
        if package.get("source").is_none() {
            let start = identity.find(" (").ok_or("local package has no path")?;
            let directory = Path::new(
                identity[start + 2..]
                    .strip_suffix(')')
                    .ok_or("invalid local package path")?,
            );
            let manifest = directory.join("Cargo.toml");
            let relative = manifest.strip_prefix(root.canonicalize()?)?.to_path_buf();
            manifests.insert(
                relative,
                toml_edit::de::from_str::<Value>(&fs::read_to_string(manifest)?)?,
            );
        }
        let features = features
            .split(',')
            .filter(|f| !f.is_empty())
            .collect::<BTreeSet<_>>();
        nodes.push(json!({"depth":depth,"package":package,"features":features}));
    }
    let config_path = root.join(".cargo/config.toml");
    let mut config: Value = if config_path.is_file() {
        toml_edit::de::from_str(&fs::read_to_string(config_path)?)?
    } else {
        json!({})
    };
    config
        .as_object_mut()
        .ok_or("invalid Cargo configuration")?
        .retain(|key, _| matches!(key.as_str(), "build" | "target" | "env"));
    Ok(json!({"nodes":nodes,"manifests":manifests,"cargo_config":config}))
}

/// Project Cargo's resolved normal/build graph, retaining shared feature unification.
/// The registry assigns direct dependencies to the mechanisms that use them.
pub fn projection(resolved: &Value, dependencies: &BTreeSet<String>) -> Result<Value> {
    let nodes = resolved["nodes"]
        .as_array()
        .ok_or("observer resolved graph missing")?;
    let mut packages = BTreeMap::<String, Value>::new();
    let mut stack = Vec::<String>::new();
    let mut active = true;
    for node in nodes {
        let depth = node["depth"]
            .as_u64()
            .ok_or("invalid observer graph depth")? as usize;
        let name = node["package"]["name"]
            .as_str()
            .ok_or("missing observer package name")?;
        if depth == 1 {
            active = dependencies.contains(name);
        }
        if depth > 0 && !active {
            continue;
        }
        stack.truncate(depth);
        let package = &node["package"];
        let key = format!(
            "{} {}{}",
            name,
            package["version"]
                .as_str()
                .ok_or("missing package version")?,
            package["source"]
                .as_str()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default()
        );
        if let Some(parent) = stack.last() {
            let edges = packages
                .get_mut(parent)
                .ok_or("observer graph parent missing")?["dependencies"]
                .as_array_mut()
                .ok_or("missing dependencies")?;
            if !edges.contains(&json!(key)) {
                edges.push(json!(key));
            }
        }
        packages.entry(key.clone()).or_insert_with(|| {
            let mut package = package.clone();
            package["dependencies"] = json!([]);
            package["features"] = node["features"].clone();
            package["script_flags"] = node["script_flags"].clone();
            package["unit_profiles"] = serde_json::json!(
                node["units"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .map(|u| serde_json::json!({"kind":u["kind"],"profile":u["profile"]}))
                    .collect::<Vec<_>>()
            );
            package
        });
        stack.push(key);
    }
    let mut manifests: BTreeMap<PathBuf, Value> =
        serde_json::from_value(resolved["manifests"].clone())?;
    let runner = manifests
        .get_mut(Path::new("hil/host/runner/Cargo.toml"))
        .ok_or("runner manifest missing")?;
    for kind in ["dependencies", "build-dependencies"] {
        if let Some(table) = runner[kind].as_object_mut() {
            table.retain(|alias, dep| {
                dependencies.contains(dep["package"].as_str().unwrap_or(alias))
            });
        }
    }
    // Helper binary declarations are not inputs to the runner executable.
    runner
        .as_object_mut()
        .ok_or("invalid runner manifest")?
        .remove("bin");
    cargo_inputs::projection(
        &json!({"package":packages.into_values().collect::<Vec<_>>()}),
        &manifests,
        &[RUNNER.into()],
    )
}

/// Registry scope for the selected workload; all direct dependencies must have an owner.
pub fn dependencies(registry: &Value, kind: &str) -> Result<BTreeSet<String>> {
    let groups = registry["dependencies"]
        .as_object()
        .ok_or("observer dependencies missing")?;
    let mut names = BTreeSet::new();
    let domain = if kind.is_empty() {
        "common"
    } else {
        registry["workload_domains"][kind]
            .as_str()
            .ok_or("observer workload domain missing")?
    };
    for group in ["common", domain] {
        for name in groups
            .get(group)
            .and_then(Value::as_array)
            .ok_or("observer dependency group missing")?
        {
            names.insert(
                name.as_str()
                    .ok_or("invalid observer dependency name")?
                    .to_owned(),
            );
        }
    }
    Ok(names)
}

pub fn validate_registry(resolved: &Value, registry: &Value) -> Result<()> {
    let assigned = registry["dependencies"]
        .as_object()
        .ok_or("observer dependency scopes missing")?
        .values()
        .flat_map(|v| v.as_array().into_iter().flatten())
        .filter_map(Value::as_str)
        .collect::<BTreeSet<_>>();
    for node in resolved["nodes"]
        .as_array()
        .ok_or("observer graph missing")?
    {
        if node["depth"] == 1
            && !assigned.contains(
                node["package"]["name"]
                    .as_str()
                    .ok_or("package name missing")?,
            )
        {
            return Err(format!(
                "observer dependency {} has no registered scope",
                node["package"]["name"]
            )
            .into());
        }
    }
    Ok(())
}

/// Profile differences are reviewed separately from dependency/feature differences.
#[allow(dead_code)]
pub fn take_unit_profiles(projection: &mut Value) -> Value {
    let mut profiles = BTreeMap::new();
    if let Some(packages) = projection["packages"].as_object_mut() {
        for (key, package) in packages {
            if let Some(profile) = package
                .as_object_mut()
                .and_then(|p| p.remove("unit_profiles"))
            {
                profiles.insert(key.clone(), profile);
            }
        }
    }
    json!(profiles)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture() -> Value {
        json!({"nodes":[
            {"depth":0,"package":{"name":RUNNER,"version":"1"},"features":[]},
            {"depth":1,"package":{"name":"wire","version":"1","source":"registry+test","checksum":"a"},"features":["std"]},
            {"depth":1,"package":{"name":"ble","version":"1","source":"registry+test","checksum":"b"},"features":[]}
        ],"manifests":{
            "Cargo.toml":{"workspace":{"members":["hil/host/runner"]}},
            "hil/host/runner/Cargo.toml":{"package":{"name":RUNNER,"version":"1"},"dependencies":{"wire":{"version":"1","features":["std"]},"ble":"1"},"dev-dependencies":{"test-only":"1"}}
        }})
    }

    #[test]
    fn wifi_scope_ignores_ble_pins_but_preserves_features_and_dependency_manifests() {
        let mut resolved = fixture();
        let scope = BTreeSet::from(["wire".into()]);
        let expected = projection(&resolved, &scope).unwrap();
        resolved["nodes"][2]["package"]["checksum"] = json!("new BLE package");
        resolved["manifests"]["hil/host/runner/Cargo.toml"]["dependencies"]["ble"] = json!("2");
        resolved["manifests"]["hil/host/runner/Cargo.toml"]["dev-dependencies"]["test-only"] =
            json!("2");
        assert_eq!(expected, projection(&resolved, &scope).unwrap());
        resolved["nodes"][1]["features"] = json!(["std", "shared-feature-enabled-by-ble"]);
        assert_ne!(expected, projection(&resolved, &scope).unwrap());
        resolved["nodes"][1]["features"] = json!(["std"]);
        resolved["manifests"]["hil/host/runner/Cargo.toml"]["dependencies"]["wire"]["features"] =
            json!(["other"]);
        assert_ne!(expected, projection(&resolved, &scope).unwrap());
    }
}
