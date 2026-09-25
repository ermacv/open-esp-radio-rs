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

use open_esp_radio_hil_schema::cargo_inputs::projection;

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
