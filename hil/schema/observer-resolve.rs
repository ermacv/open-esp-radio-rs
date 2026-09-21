//! Producer-only Cargo graph resolution. Evaluators read prepared descriptors.
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
    process::Command,
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
const RUNNER: &str = "open-esp-radio-hil-runner";

pub fn resolve(root: &Path, target: &str) -> Result<Value> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args([
            "tree",
            "--locked",
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
