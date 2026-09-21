//! Canonical per-comparison inputs shared by the producer and independent consumer.
//! This describes identity only; neither release eligibility nor MATCH is inferred.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
};

#[path = "model-inputs.rs"]
pub mod models;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct Binding {
    pub project_manifest: PathBuf,
    pub sha256: String,
}

pub struct Current {
    pub sha256: String,
    #[allow(dead_code)] // Producer compares the actual loaded/compiled context.
    pub model_context: Value,
    pub component: Option<String>,
    pub baseline_digests: Vec<String>,
    pub artifact_pins: BTreeMap<String, String>,
    pub public_artifacts_bound: bool,
    #[allow(dead_code)] // Producer guards publication against edits during execution.
    pub input_hashes: BTreeMap<String, String>,
}

pub fn current(
    root: &Path,
    project: &Path,
    suite_id: &str,
    source: &str,
    symbol: &str,
    artifacts: &BTreeMap<String, String>,
) -> Result<Current> {
    let path = contained(root, &root.join(project))?;
    let project = read(&path)?;
    let addon_path = contained(
        root,
        &path
            .parent()
            .unwrap()
            .join(string(&project, "verification-addon")?),
    )?;
    let addon = read(&addon_path)?;
    let base = addon_path.parent().unwrap();
    let suite = array(&addon, "suites")
        .iter()
        .find(|s| s["id"] == suite_id)
        .ok_or("comparison suite missing")?;
    let selected = array(suite, "vendor").iter().any(|v| {
        v["source"] == source
            && (v["all"] == true
                || array(v, "symbols").iter().any(|s| s == symbol)
                || v["prefix"].as_str().is_some_and(|p| symbol.starts_with(p)))
    });
    if !selected {
        return Err("comparison source/symbol is no longer selected".into());
    }
    let mut input_hashes = BTreeMap::new();
    let mut dispositions = Vec::new();
    let mut profiles = Vec::new();
    let mut baselines = Vec::new();
    for (key, rows, output) in [
        ("dispositions", "functions", &mut dispositions),
        ("profiles", "profiles", &mut profiles),
        ("baselines", "evidence", &mut baselines),
    ] {
        for (index, relative) in array(suite, key).iter().enumerate() {
            let input_path = contained(
                root,
                &base.join(relative.as_str().ok_or("invalid comparison input path")?),
            )?;
            let role = if key == "baselines" {
                "evidence-baseline"
            } else {
                key
            };
            let bytes = fs::read(&input_path)?;
            input_hashes.insert(
                format!("{role}:{index}"),
                format!("{:x}", Sha256::digest(&bytes)),
            );
            let doc: Value = toml_edit::de::from_str(std::str::from_utf8(&bytes)?)?;
            let mut header = doc.clone();
            header
                .as_object_mut()
                .ok_or("invalid comparison document")?
                .remove(rows);
            let rows = array(&doc, rows)
                .iter()
                .filter(|row| {
                    let (source_key, symbol_key) = if key == "profiles" {
                        ("vendor-source", "vendor-symbol")
                    } else {
                        ("source", "symbol")
                    };
                    row[source_key] == source && row[symbol_key] == symbol
                })
                .cloned()
                .collect::<Vec<_>>();
            output.push(json!({"header":header,"rows":rows}));
        }
    }
    let functions = dispositions
        .iter()
        .flat_map(|d| array(d, "rows"))
        .collect::<Vec<_>>();
    let function = functions
        .first()
        .ok_or("comparison production disposition missing")?;
    if functions
        .iter()
        .any(|f| strip_annotations((*f).clone()) != strip_annotations((*function).clone()))
    {
        return Err("conflicting comparison dispositions".into());
    }
    let component = function["rust-component"].as_str().map(str::to_owned);
    let baseline_digests = baselines
        .iter()
        .flat_map(|d| array(d, "rows"))
        .filter_map(|b| b["digest"].as_str().map(str::to_owned))
        .collect();
    let mut policy = Value::Null;
    if let Some(relative) = addon["policy"].as_str() {
        let doc = read(&contained(root, &base.join(relative))?)?;
        let surfaces = array(&doc, "surfaces")
            .iter()
            .filter(|surface| {
                array(surface, "requirements").iter().any(|r| {
                    r["suite"] == suite_id && r["source"] == source && r["symbol"] == symbol
                })
            })
            .cloned()
            .collect::<Vec<_>>();
        policy = json!({"schema":doc["schema"],"surfaces":surfaces});
    }
    let mut artifact_pins = BTreeMap::new();
    for vendor in array(suite, "vendor") {
        for (key, role) in [
            ("artifact-sha256", "artifact"),
            ("companion-sha256", "companion"),
        ] {
            if let Some(hash) = vendor[key].as_str() {
                artifact_pins.insert(
                    format!("source:{}:{role}", string(vendor, "source")?),
                    hash.to_owned(),
                );
            }
        }
    }
    if let Some(pins) = suite.get("artifact-bindings") {
        let pins = pins
            .as_object()
            .ok_or("artifact-bindings must be a table")?;
        for (role, hash) in pins {
            let hash = hash
                .as_str()
                .ok_or("artifact binding must be a SHA-256 string")?;
            if !(role.starts_with("source:") || role.starts_with("auxiliary:"))
                || hash.len() != 64
                || !hash
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
            {
                return Err("invalid public artifact binding".into());
            }
            if artifact_pins
                .insert(role.clone(), hash.to_owned())
                .is_some_and(|old| old != hash)
            {
                return Err("conflicting public artifact bindings".into());
            }
        }
    }
    let public_artifacts_bound = artifacts.contains_key(&format!("source:{source}:artifact"))
        && artifacts
            .iter()
            .filter(|(role, _)| role.starts_with("source:") || role.starts_with("auxiliary:"))
            .all(|(role, hash)| artifact_pins.get(role) == Some(hash))
        && artifact_pins
            .iter()
            .all(|(role, hash)| artifacts.get(role) == Some(hash));
    let model_context = models::current(root, &path, &project, &addon_path, &addon, suite)?;
    let identity = strip_annotations(
        json!({"format":"oer-vendor-comparison-v2", "model_context":model_context, "project":project["id"],
        "suite":suite,"source":source,"symbol":symbol,"dispositions":dispositions,
        "profiles":profiles,"baselines":baselines,"policy":policy,"artifacts":artifacts}),
    );
    Ok(Current {
        sha256: format!("{:x}", Sha256::digest(serde_json::to_vec(&identity)?)),
        model_context,
        component,
        baseline_digests,
        artifact_pins,
        public_artifacts_bound,
        input_hashes,
    })
}

fn strip_annotations(mut value: Value) -> Value {
    match &mut value {
        Value::Object(object) => {
            for key in ["description", "notes", "rationale", "reason", "title"] {
                object.remove(key);
            }
            for item in object.values_mut() {
                *item = strip_annotations(item.take());
            }
        }
        Value::Array(items) => {
            for item in items {
                *item = strip_annotations(item.take());
            }
        }
        _ => {}
    }
    value
}
fn array<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value[key].as_array().map(Vec::as_slice).unwrap_or(&[])
}
fn string<'a>(value: &'a Value, key: &str) -> Result<&'a str> {
    value[key]
        .as_str()
        .ok_or_else(|| format!("missing comparison field {key}").into())
}
fn read(path: &Path) -> Result<Value> {
    Ok(toml_edit::de::from_str(&fs::read_to_string(path)?)?)
}
fn contained(root: &Path, path: &Path) -> Result<PathBuf> {
    let path = path.canonicalize()?;
    if !path.starts_with(root.canonicalize()?) || !path.is_file() {
        return Err("comparison input escapes repository".into());
    }
    Ok(path)
}
