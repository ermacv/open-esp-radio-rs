//! Host build inputs shared by the runner producer and independent evaluator.
use crate::cargo_inputs;
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const RUNNER: &str = "open-esp-radio-hil-runner";

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

/// The observer build configuration an evaluator requires on this host: the
/// current compiler and the registry's build profile and flags. A producer's
/// embedded configuration must equal it for its evidence to be current.
pub fn required_configuration(root: &Path, registry: &Value) -> Result<Value> {
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

/// Profile differences are reviewed separately from dependency/feature differences.
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
