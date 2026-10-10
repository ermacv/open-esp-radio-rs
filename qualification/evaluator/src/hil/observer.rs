//! The executed host observer has its own build subject, separate from firmware.
use super::*;
pub(super) use oer_hil_run_bundle_format::observer::inputs as build_inputs;
use serde_json::{Value, json};

/// One prepared configuration shared by archive loading and shards. Loading it
/// only reads files; producer preparation is an explicit xtask operation.
///
/// Everything an assessment needs of the current observer depends on the
/// workload alone, never on the observation: it is computed once per
/// evaluation, and the prepared graph itself, hundreds of megabytes once
/// parsed, is not kept.
#[derive(Debug, Default)]
pub(super) struct Current {
    prepared: Option<Prepared>,
    pub(super) problem: Option<String>,
}

#[derive(Debug)]
struct Prepared {
    /// What an observer build must record besides its unit profiles: the
    /// compiler, environment, Cargo configuration and selected profiles.
    configuration: Value,
    /// The current side of each workload the registry classifies, and of
    /// the unscoped workload `""`.
    workloads: BTreeMap<String, Workload>,
}

#[derive(Debug)]
struct Workload {
    dependencies: BTreeSet<String>,
    /// The current graph projected onto the workload's dependencies, without
    /// its unit profiles and workspace profile.
    projection: Value,
    units: Value,
    /// The workload's source inputs and their digests, hashed on first use.
    inputs: std::cell::OnceCell<std::result::Result<Inputs, String>>,
}

#[derive(Debug)]
struct Inputs {
    prefixes: Vec<PathBuf>,
    digests: BTreeMap<String, String>,
}

impl Workload {
    fn new(resolved: &Value, dependencies: BTreeSet<String>) -> Result<Self> {
        let mut projection = build_inputs::projection(resolved, &dependencies)?;
        let units = build_inputs::take_unit_profiles(&mut projection);
        without_workspace_profile(&mut projection)?;
        Ok(Self {
            dependencies,
            projection,
            units,
            inputs: std::cell::OnceCell::new(),
        })
    }

    fn inputs(&self, root: &Path) -> Result<&Inputs> {
        self.inputs
            .get_or_init(|| self.hash_inputs(root).map_err(|error| error.to_string()))
            .as_ref()
            .map_err(|error| error.clone().into())
    }

    fn hash_inputs(&self, root: &Path) -> Result<Inputs> {
        #[cfg(test)]
        work::count(|work| work.input_hashes += 1);
        let prefixes = source_inputs(root, &self.projection)?;
        // Used manifests are compared through the shared Cargo projection.
        // Their raw bytes remain provenance, but dev-only declarations must
        // not reintroduce an unrelated dependency through the file selector.
        let mut digests = inputs(root, &prefixes)?;
        digests.retain(|path, _| !self.projected_manifest(path));
        Ok(Inputs { prefixes, digests })
    }

    fn projected_manifest(&self, path: &str) -> bool {
        self.projection["manifests"].get(path).is_some()
    }
}

fn without_workspace_profile(projection: &mut Value) -> Result<()> {
    projection["workspace"]
        .as_object_mut()
        .ok_or("workspace missing")?
        .remove("profile");
    Ok(())
}

impl Current {
    pub(super) fn load(root: &Path) -> Self {
        match Self::read(root) {
            Ok(current) => current,
            Err(error) => Self {
                problem: Some(error.to_string()),
                ..Self::default()
            },
        }
    }
    fn read(root: &Path) -> Result<Self> {
        let path = oer_hil_run_bundle_format::observer::receipt::selected(root);
        let mut receipt: Value = read_json(&path).map_err(|error| format!(
            "current observer configuration unavailable ({}): {error}; prepare with cargo hil observer", path.display()))?;
        // The graph is hundreds of megabytes once parsed: it is taken out of
        // the receipt and turned into the live graph in place, never copied.
        let mut build = receipt["build"].take();
        drop(receipt);
        let mut resolved = build["resolved"].take();
        if build["schema"] != 2
            || resolved["compilation"] != "cargo-compiler-artifacts-v1"
            || resolved["selected_profile"].as_str().is_none()
        {
            return Err("prepared observer configuration is invalid".into());
        }
        resolved["configuration"] =
            json!({"compiler":build["compiler"],"environment":build["environment"]});
        drop(build);
        let registry: Value = read_json(&root.join("hil/schema/observer-inputs.json"))?;
        build_inputs::check_registry_schema(&registry)?;
        build_inputs::validate_registry(&resolved, &registry)?;
        let workloads = std::iter::once(String::new())
            .chain(build_inputs::workloads(&registry)?)
            .map(|kind| {
                #[cfg(test)]
                work::count(|work| work.current_projections += 1);
                let dependencies = build_inputs::dependencies(&registry, &kind)?;
                Ok((kind, Workload::new(&resolved, dependencies)?))
            })
            .collect::<Result<BTreeMap<_, _>>>()?;
        let mut configuration = resolved["configuration"].clone();
        configuration["cargo"] = resolved["cargo_config"].clone();
        configuration["profiles"] = profile_configuration(&resolved);
        let mut manifests = resolved["manifests"]
            .as_object()
            .ok_or("observer manifests missing")?
            .clone();
        for (path, manifest) in &mut manifests {
            if !safe_relative(Path::new(path)) {
                return Err("unsafe observer manifest path".into());
            }
            *manifest = toml_edit::de::from_str(&fs::read_to_string(root.join(path))?)?;
        }
        let config = root.join(".cargo/config.toml");
        let mut config: Value = if config.is_file() {
            toml_edit::de::from_str(&fs::read_to_string(config)?)?
        } else {
            json!({})
        };
        config
            .as_object_mut()
            .ok_or("invalid Cargo config")?
            .retain(|k, _| matches!(k.as_str(), "build" | "target" | "env"));
        // Any normal/build dependency can participate in feature unification.
        // A prepared descriptor remains a current configuration only while the
        // full normal/build declarations match; source compatibility is scoped
        // separately below and never requires compiling another domain.
        let mut dependencies = registry["dependencies"]
            .as_object()
            .ok_or("observer dependency scopes missing")?
            .values()
            .flat_map(|v| v.as_array().into_iter().flatten())
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect::<BTreeSet<_>>();
        // Keep new, not-yet-registered direct dependencies in this freshness
        // comparison too; the producer must resolve and assign them explicitly.
        for kind in ["dependencies", "build-dependencies"] {
            for (alias, dependency) in manifests
                .get("hil/host/runner/Cargo.toml")
                .and_then(|manifest| manifest[kind].as_object())
                .into_iter()
                .flatten()
            {
                dependencies.insert(dependency["package"].as_str().unwrap_or(alias).to_owned());
            }
        }
        let profile = registry["build"]["profile"]
            .as_str()
            .ok_or("observer profile missing")?;
        let profile = if profile == "debug" { "dev" } else { profile };
        let prepared_projection = build_inputs::projection(&resolved, &dependencies)?;
        let prepared_lock = lock_dependencies(&resolved)?;
        let fresh_configuration =
            resolved["cargo_config"] == config && resolved["selected_profile"] == profile;
        // The live graph: the prepared one with today's manifests, locked
        // packages and Cargo configuration.
        let mut live = resolved;
        live["manifests"] = Value::Object(manifests);
        let lock: Value = toml_edit::de::from_str(&fs::read_to_string(root.join("Cargo.lock"))?)?;
        let packages = lock["package"].as_array().ok_or("lock packages missing")?;
        for node in live["nodes"]
            .as_array_mut()
            .ok_or("observer graph missing")?
        {
            let old = &node["package"];
            node["package"] = packages.iter().find(|p| p["name"] == old["name"] && p["version"] == old["version"] && p["source"] == old["source"])
                .cloned().unwrap_or_else(|| json!({"name":old["name"],"version":old["version"],"source":old["source"],"missing":true}));
        }
        live["cargo_config"] = config;
        if prepared_projection != build_inputs::projection(&live, &dependencies)?
            || prepared_lock != lock_dependencies(&live)?
            || !fresh_configuration
        {
            return Err("prepared observer configuration is stale: normal/build dependencies or Cargo configuration changed; prepare with cargo hil observer".into());
        }
        Ok(Self {
            prepared: Some(Prepared {
                configuration,
                workloads,
            }),
            problem: None,
        })
    }
    pub(super) fn available(&self) -> bool {
        self.prepared.is_some()
    }

    /// Whether `proof`, an observer record with its build inline whose
    /// identity [`identified`] accepted, built the current observer of
    /// `workload`.
    pub(super) fn assess(
        &self,
        root: &Path,
        proof: &Value,
        workload: &str,
    ) -> Result<Compatibility> {
        let Some(current) = &self.prepared else {
            return Ok(Compatibility::IdentityDiffers);
        };
        // A run of a workload the registry no longer classifies, such as one
        // of a removed scenario, has no dependency scope in today's observer
        // inputs.
        let Some(workload) = current.workloads.get(workload) else {
            return Ok(Compatibility::GraphNotProjectable);
        };
        #[cfg(test)]
        work::count(|work| work.recorded_projections += 1);
        // An observer recorded by an older runner whose package graph no
        // longer projects onto today's inputs cannot be this observer: the
        // observation is excluded as incompatible, as for any other
        // dependency difference.
        let Ok(mut old_dependencies) =
            build_inputs::projection(&proof["build"]["resolved"], &workload.dependencies)
        else {
            return Ok(Compatibility::GraphNotProjectable);
        };
        let old_units = build_inputs::take_unit_profiles(&mut old_dependencies);
        without_workspace_profile(&mut old_dependencies)?;
        if old_dependencies != workload.projection {
            return Ok(Compatibility::IdentityDiffers);
        }
        let inputs = workload.inputs(root)?;
        let Some(recorded) = proof["build"]["inputs"].as_object() else {
            return Ok(Compatibility::IdentityDiffers);
        };
        let relevant = recorded
            .iter()
            .filter(|(path, _)| {
                selected(Path::new(path), &inputs.prefixes) && !workload.projected_manifest(path)
            })
            .map(|(path, hash)| (path.clone(), hash.as_str().unwrap_or_default().to_owned()))
            .collect::<BTreeMap<_, _>>();
        if inputs.digests != relevant {
            return Ok(Compatibility::IdentityDiffers);
        }
        let actual = json!({"units":old_units,"cargo":proof["build"]["resolved"]["cargo_config"],"compiler":proof["build"]["compiler"],"environment":proof["build"]["environment"],"profiles":profile_configuration(&proof["build"]["resolved"])});
        let mut required = current.configuration.clone();
        required["units"] = workload.units.clone();
        Ok(if actual == required {
            Compatibility::Compatible
        } else {
            Compatibility::IdentityDiffers
        })
    }
}

/// Whether `proof`, an observer record with its build inline, is a complete
/// identity: the build it names, compiled from resolved Cargo units under a
/// selected profile, hashes to the digest the record names. A record that
/// is not cannot identify the current observer for any workload.
pub(super) fn identified(proof: &Value) -> Result<bool> {
    Ok(proof["schema"] == 1
        && proof["build"]["schema"] == 2
        && proof["build"]["resolved"]["compilation"] == "cargo-compiler-artifacts-v1"
        && proof["build"]["resolved"]["selected_profile"]
            .as_str()
            .is_some_and(|profile| !profile.is_empty())
        && proof["executable_sha256"]
            .as_str()
            .is_some_and(valid_sha256)
        && proof["build_sha256"].as_str()
            == Some(&oer_durable::sha256_bytes(&serde_json::to_vec(
                &proof["build"],
            )?)))
}

/// The workload an observation's executed procedure ran, `""` for one that
/// names none.
pub(super) fn workload(observation: &ScenarioEvidence) -> Result<String> {
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
    Ok(document
        .as_ref()
        .and_then(build_inputs::workload)
        .unwrap_or_default())
}

/// Counts of the work assessments do, which regression tests bound.
#[cfg(test)]
pub(super) mod work {
    use std::cell::Cell;

    #[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
    pub(in crate::hil) struct Work {
        /// Projections of the current observer's graph.
        pub(in crate::hil) current_projections: usize,
        /// Hashings of a workload's current source inputs.
        pub(in crate::hil) input_hashes: usize,
        /// Observer builds read from a store.
        pub(in crate::hil) build_loads: usize,
        /// Projections of a recorded observer build's graph.
        pub(in crate::hil) recorded_projections: usize,
    }

    thread_local! {
        static WORK: Cell<Work> = Cell::new(Work::default());
    }

    pub(in crate::hil) fn count(update: impl FnOnce(&mut Work)) {
        WORK.with(|cell| {
            let mut work = cell.get();
            update(&mut work);
            cell.set(work);
        });
    }

    /// The work counted on this thread since the last call.
    pub(in crate::hil) fn take() -> Work {
        WORK.with(Cell::take)
    }
}

// Cargo can retain both old and new versions for other workspace consumers.
// Compare the recorded lock edges too, rather than merely finding the old nodes.
fn lock_dependencies(resolved: &Value) -> Result<Value> {
    let manifests = resolved["manifests"]
        .as_object()
        .ok_or("observer manifests missing")?;
    let mut packages = BTreeMap::new();
    for node in resolved["nodes"]
        .as_array()
        .ok_or("observer nodes missing")?
    {
        let package = &node["package"];
        let allowed = if package.get("source").is_none() {
            let manifest = manifests
                .values()
                .find(|m| m["package"]["name"] == package["name"])
                .ok_or("local dependency manifest missing")?;
            Some(
                oer_hil_run_bundle_format::observer::cargo_inputs::dependency_names_in(
                    manifest,
                    &resolved["manifests"]["Cargo.toml"],
                )?,
            )
        } else {
            None
        };
        let dependencies = package["dependencies"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|d| {
                allowed.as_ref().is_none_or(|names| {
                    names.contains(d.split_whitespace().next().unwrap_or_default())
                })
            })
            .collect::<BTreeSet<_>>();
        packages.insert(
            serde_json::to_string(&json!([
                package["name"],
                package["version"],
                package["source"]
            ]))?,
            dependencies,
        );
    }
    Ok(json!(packages))
}

/// A workload's source inputs: the registry's data files and, for the runner
/// and every path package in the projected dependency closure, its sources,
/// build script and manifest.
fn source_inputs(root: &Path, projection: &Value) -> Result<Vec<PathBuf>> {
    let mut prefixes = data_inputs(root)?;
    for path in projection["manifests"]
        .as_object()
        .ok_or("observer manifests missing")?
        .keys()
    {
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
    Ok(prefixes)
}

/// Non-Cargo observer inputs; Cargo packages are selected from the projected
/// dependency closure instead.
fn data_inputs(root: &Path) -> Result<Vec<PathBuf>> {
    let registry: Value = read_json(&root.join("hil/schema/observer-inputs.json"))?;
    build_inputs::check_registry_schema(&registry)?;
    registry["data"]
        .as_array()
        .ok_or("observer data inputs missing")?
        .iter()
        .map(|input| {
            let path = PathBuf::from(input.as_str().ok_or("invalid observer input")?);
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

/// The regular files below `directory`, relative to it and sorted; a
/// symlink or special file is an error.
fn regular_files(directory: &Path) -> Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    let mut pending = vec![PathBuf::new()];
    while let Some(relative) = pending.pop() {
        for entry in fs::read_dir(directory.join(&relative))? {
            let entry = entry?;
            let kind = entry.file_type()?;
            let child = relative.join(entry.file_name());
            if kind.is_dir() {
                pending.push(child);
            } else if kind.is_file() {
                found.push(child);
            } else {
                return Err(format!(
                    "observer input contains a symlink or special file: {}",
                    entry.path().display()
                )
                .into());
            }
        }
    }
    found.sort();
    Ok(found)
}

fn inputs(root: &Path, prefixes: &[PathBuf]) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    for relative in prefixes {
        let path = root.join(relative);
        if fs::symlink_metadata(&path)?.file_type().is_dir() {
            for child in regular_files(&path)? {
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
                        crate::digests()
                            .sha256_file(&root.join(relative))
                            .map_err(|error| error.to_string())?,
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

/// Why an observation's observer is or is not the current observer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Compatibility {
    Compatible,
    /// The recorded package graph, from an older runner, does not project
    /// onto today's observer inputs.
    GraphNotProjectable,
    IdentityDiffers,
}

#[cfg(test)]
pub(super) use oer_hil_run_bundle_format::observer::inputs::required_configuration;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lock_reselection_cannot_reuse_an_old_node_retained_for_another_consumer() {
        let mut resolved = json!({"manifests":{"Cargo.toml":{},"local/Cargo.toml":{"package":{"name":"local"},"dependencies":{"dep":"1"},"dev-dependencies":{"test-only":"1"}}},"nodes":[
            {"package":{"name":"local","version":"1","dependencies":["dep 1","test-only 1"]}},
            {"package":{"name":"dep","version":"1","source":"registry+test","dependencies":["transitive 1"]}},
            {"package":{"name":"transitive","version":"1","source":"registry+test"}}
        ]});
        let before = lock_dependencies(&resolved).unwrap();
        resolved["nodes"][0]["package"]["dependencies"] = json!(["dep 1", "test-only 2"]);
        assert_eq!(before, lock_dependencies(&resolved).unwrap());
        resolved["nodes"][1]["package"]["dependencies"] = json!(["transitive 2"]);
        assert_ne!(before, lock_dependencies(&resolved).unwrap());
    }

    #[test]
    fn catalog_workloads_select_only_their_family_packages() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let registry: Value = read_json(&root.join("hil/schema/observer-inputs.json")).unwrap();
        let catalog = ScenarioCatalog::load(&root, Path::new("hil/scenarios")).unwrap();
        let family = |kind: &str| {
            let names = build_inputs::dependencies(&registry, kind).unwrap();
            [
                "oer-hil-family-ieee80211",
                "oer-hil-family-bluetooth",
                "oer-hil-family-system",
                "oer-hil-family-ieee802154",
                "oer-hil-family-phy-esp32s31",
            ]
            .into_iter()
            .filter(|package| names.contains(*package))
            .collect::<Vec<_>>()
        };
        let mut used = std::collections::BTreeSet::new();
        for document in catalog.definitions.values() {
            let workload = build_inputs::workload(document).unwrap();
            used.insert(workload.clone());
            let kind = workload.as_str();
            let packages = family(kind);
            if kind == build_inputs::COEXISTENCE_WORKLOAD {
                assert_eq!(
                    packages,
                    ["oer-hil-family-ieee80211", "oer-hil-family-bluetooth"],
                    "the joint workload selects both radio families"
                );
            } else {
                assert_eq!(
                    packages.len(),
                    1,
                    "{kind} selects exactly one family: {packages:?}"
                );
            }
            assert!(
                build_inputs::dependencies(&registry, kind)
                    .unwrap()
                    .contains("oer-hil-workload")
            );
        }
        // Every classified workload is some catalog scenario's: a stale
        // entry would keep scoping inputs no run reads.
        for workload in build_inputs::workloads(&registry).unwrap() {
            assert!(
                used.contains(&workload),
                "{workload} is no catalog scenario's workload"
            );
        }
        assert_eq!(family("wifi/station-udp"), ["oer-hil-family-ieee80211"]);
        assert_eq!(family("bluetooth/dtm"), ["oer-hil-family-bluetooth"]);
        assert_eq!(family("system/boot-smoke"), ["oer-hil-family-system"]);
        assert_eq!(
            family("phy/vendor-calibration"),
            ["oer-hil-family-phy-esp32s31"]
        );
        assert!(!data_inputs(&root).unwrap().is_empty());
    }

    #[test]
    fn projected_packages_and_data_are_the_only_source_inputs() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let projection = json!({"manifests": {
            "hil/host/runner/Cargo.toml": {},
            "hil/host/family/ieee80211/Cargo.toml": {},
        }});
        let prefixes = source_inputs(&root, &projection).unwrap();
        let wifi = Path::new("hil/host/family/ieee80211/src/workload/traffic/rx_traffic.rs");
        assert!(selected(wifi, &prefixes));
        assert!(selected(
            Path::new("hil/host/runner/src/execution.rs"),
            &prefixes
        ));
        assert!(selected(
            Path::new("hil/schema/scenario-v5-defaults.json"),
            &prefixes
        ));
        assert!(!selected(
            Path::new("hil/host/family/bluetooth/src/workload/bluetooth/gatt.rs"),
            &prefixes
        ));
        assert!(!selected(
            Path::new("hil/host/family/ieee80211/src/workload/traffic/bidirectional/tests.rs"),
            &prefixes
        ));
        assert!(!selected(Path::new("Cargo.lock"), &prefixes));
    }
}
