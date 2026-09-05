//! Cargo resolution preserves workspace isolation and the original lock catalog.

use crate::{
    Context, Result,
    graph::{self, Graph},
    process,
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    path::{Path, PathBuf},
};

pub fn workspace_manifest(context: &Context, manifest: &Path) -> Result<PathBuf> {
    let output = process::capture(
        context
            .cargo()
            .args([
                "locate-project",
                "--workspace",
                "--message-format",
                "json",
                "--manifest-path",
            ])
            .arg(manifest),
    )?;
    let document: Value = serde_json::from_slice(&output.stdout)?;
    let root = document
        .get("root")
        .and_then(Value::as_str)
        .ok_or("workspace root missing")?;
    let root = Path::new(root);
    if !root.is_absolute() {
        return Err("workspace manifest path must be absolute".into());
    }
    Ok(root.canonicalize()?)
}

fn document(
    context: &Context,
    manifest: &Path,
    features: &[String],
    target: Option<&str>,
    locked: bool,
    no_deps: bool,
) -> Result<Value> {
    let mut command = context.cargo();
    command
        .args(["metadata", "--format-version", "1", "--manifest-path"])
        .arg(manifest)
        .args(features);
    command.arg("--offline");
    if locked {
        command.arg("--locked");
    }
    if let Some(target) = target {
        command.args(["--filter-platform", target]);
    }
    if no_deps {
        command.arg("--no-deps");
    }
    Ok(serde_json::from_slice(
        &process::capture(&mut command)?.stdout,
    )?)
}
pub fn metadata(
    context: &Context,
    manifest: &Path,
    features: &[String],
    target: Option<&str>,
    locked: bool,
) -> Result<Graph> {
    Graph::from_value(document(
        context, manifest, features, target, locked, false,
    )?)
}
pub fn metadata_no_deps(context: &Context, manifest: &Path) -> Result<cargo_metadata::Metadata> {
    let value = document(context, manifest, &[], None, true, true)?;
    graph::validate_packages(&value)?;
    Ok(serde_json::from_value(value)?)
}

fn table(value: &toml::Value) -> Result<&toml::map::Map<String, toml::Value>> {
    value
        .as_table()
        .ok_or_else(|| "expected Cargo TOML table".into())
}
fn string(value: Option<&toml::Value>) -> Result<&str> {
    value
        .and_then(toml::Value::as_str)
        .ok_or_else(|| "expected Cargo TOML string".into())
}
fn rebase(specification: &mut toml::Value, base: &Path) -> Result<()> {
    let spec = specification
        .as_table_mut()
        .ok_or("invalid Cargo override specification")?;
    if let Some(path) = spec.get_mut("path") {
        let absolute = base.join(string(Some(path))?).canonicalize()?;
        *path = toml::Value::String(
            absolute
                .to_str()
                .ok_or("Cargo TOML path is not Unicode")?
                .to_owned(),
        );
    }
    Ok(())
}

pub fn isolated_graph(
    context: &Context,
    manifest: &Path,
    features: &[String],
    target: Option<&str>,
) -> Result<Graph> {
    let manifest = manifest.canonicalize()?;
    let origin = workspace_manifest(context, &manifest)?;
    let origin_base = origin.parent().ok_or("workspace parent missing")?;
    let original: toml::Value = toml::from_str(&fs::read_to_string(&origin)?)?;
    let package: toml::Value = toml::from_str(&fs::read_to_string(&manifest)?)?;
    let name = string(package.get("package").and_then(|p| p.get("name")))?;
    let lock_text = fs::read_to_string(origin_base.join("Cargo.lock"))?;
    let lock: toml::Value = toml::from_str(&lock_text)?;
    let mut pins = BTreeSet::new();
    for entry in lock
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or("origin lock package catalog missing")?
    {
        pins.insert((
            string(entry.get("name"))?.to_owned(),
            string(entry.get("version"))?.to_owned(),
            entry
                .get("source")
                .map(|v| string(Some(v)).map(str::to_owned))
                .transpose()?,
        ));
    }
    let probe = "oer-network-boundary-probe";
    if pins.iter().any(|pin| pin.0 == probe) {
        return Err("probe package conflicts with origin lock".into());
    }
    let mut selected = Vec::new();
    let mut defaults = true;
    let mut flags = features.iter();
    while let Some(flag) = flags.next() {
        match flag.as_str() {
            "--no-default-features" => defaults = false,
            "--features" => selected.extend(
                flags
                    .next()
                    .ok_or("--features requires a value")?
                    .split(',')
                    .map(|v| toml::Value::String(v.to_owned())),
            ),
            _ => return Err(format!("unsupported isolated feature option: {flag}").into()),
        }
    }
    let mut consumer: toml::Value = toml::from_str(&format!(
        "[package]\nname = {probe:?}\nversion = \"0.0.0\"\nedition = \"2024\"\n[workspace]\nresolver = \"3\"\n[dependencies]\n"
    ))?;
    let dependency = toml::map::Map::from_iter([
        ("package".into(), toml::Value::String(name.into())),
        (
            "path".into(),
            toml::Value::String(
                manifest
                    .parent()
                    .unwrap()
                    .to_str()
                    .ok_or("Cargo path is not Unicode")?
                    .into(),
            ),
        ),
        ("default-features".into(), toml::Value::Boolean(defaults)),
        ("features".into(), toml::Value::Array(selected)),
    ]);
    consumer["dependencies"]
        .as_table_mut()
        .unwrap()
        .insert("audited".into(), toml::Value::Table(dependency));
    for key in ["patch", "replace"] {
        if let Some(mut overrides) = original.get(key).cloned() {
            if key == "patch" {
                for entries in overrides
                    .as_table_mut()
                    .ok_or("invalid workspace patch table")?
                    .iter_mut()
                    .map(|(_, value)| value)
                {
                    for spec in entries
                        .as_table_mut()
                        .ok_or("invalid workspace patch entries")?
                        .iter_mut()
                        .map(|(_, value)| value)
                    {
                        rebase(spec, origin_base)?;
                    }
                }
            } else {
                for spec in overrides
                    .as_table_mut()
                    .ok_or("invalid workspace replace table")?
                    .iter_mut()
                    .map(|(_, value)| value)
                {
                    rebase(spec, origin_base)?;
                }
            }
            consumer
                .as_table_mut()
                .unwrap()
                .insert(key.into(), overrides);
        }
    }
    let temporary = tempfile::Builder::new()
        .prefix("oer-network-metadata-")
        .tempdir()?;
    let scratch = temporary.path().join("Cargo.toml");
    fs::create_dir(temporary.path().join("src"))?;
    fs::write(temporary.path().join("src/lib.rs"), "")?;
    fs::write(temporary.path().join("Cargo.lock"), &lock_text)?;
    fs::write(&scratch, toml::to_string(&consumer)?)?;
    let context = Context {
        root: origin_base.to_owned(),
        cargo: context.cargo.clone(),
    };
    let resolve = |locked| -> Result<Graph> {
        let graph = metadata(&context, &scratch, &[], target, locked)?;
        let probe_id = graph.root(&scratch)?;
        for p in &graph.metadata.packages {
            if p.id == probe_id {
                continue;
            }
            let pin = (
                p.name.to_string(),
                p.version.to_string(),
                p.source.as_ref().map(ToString::to_string),
            );
            if !pins.contains(&pin) {
                return Err(
                    format!("isolated dependency drifted from origin lock: {pin:?}").into(),
                );
            }
        }
        Ok(graph)
    };
    let mut graph = resolve(false)?;
    let scratch_lock: toml::Value =
        toml::from_str(&fs::read_to_string(temporary.path().join("Cargo.lock"))?)?;
    if let Some(unused) = scratch_lock
        .get("patch")
        .and_then(|p| p.get("unused"))
        .and_then(toml::Value::as_array)
        .filter(|v| !v.is_empty())
    {
        let names = unused
            .iter()
            .map(|entry| string(entry.get("name")).map(str::to_owned))
            .collect::<Result<BTreeSet<_>>>()?;
        let mut found = BTreeMap::<String, Vec<(String, String)>>::new();
        if let Some(patches) = consumer.get("patch") {
            for (source, entries) in table(patches)? {
                for (key, spec) in table(entries)? {
                    let name = spec
                        .get("package")
                        .map(|v| string(Some(v)))
                        .transpose()?
                        .unwrap_or(key);
                    if names.contains(name) {
                        found
                            .entry(name.into())
                            .or_default()
                            .push((source.clone(), key.clone()));
                    }
                }
            }
        }
        for name in names {
            let matches = found
                .get(&name)
                .filter(|v| v.len() == 1)
                .ok_or_else(|| format!("cannot uniquely identify unused patch: {name}"))?;
            let (source, key) = &matches[0];
            consumer["patch"][source]
                .as_table_mut()
                .unwrap()
                .remove(key);
        }
        fs::write(&scratch, toml::to_string(&consumer)?)?;
        let pruned = resolve(false)?;
        if !graph.same_resolution(&pruned) {
            return Err("removing an unused patch changed resolved dependencies".into());
        }
        graph = pruned;
    }
    let locked = resolve(true)?;
    if !graph.same_resolution(&locked) {
        return Err("locked dependency resolution changed after initialization".into());
    }
    locked.root(&manifest)?;
    Ok(locked)
}
