//! The files of a checkout that can change a HIL observation.
//!
//! A run's source snapshot binds the whole repository, but a firmware image
//! and the host runner read only a part of it: the path packages the firmware
//! workspaces build, the path packages the runner depends on, the workspace
//! and toolchain files that configure both builds, the platform's placement
//! inputs and the scenario catalog. A snapshot that matches the checkout in
//! every such file is current even when unrelated files (documentation,
//! other crates) changed since the run. The closure is taken from the
//! checkout being evaluated, so a package that joined it since the run has
//! no files in the snapshot and makes the snapshot stale.

use super::*;
use serde_json::Value;
use std::process::Command;

/// Workspaces whose path packages a firmware image builds.
const FIRMWARE_WORKSPACES: [&str; 2] = [
    "hil/targets/esp32s31/Cargo.toml",
    "platform/esp32s31/bootstrap/Cargo.toml",
];

/// The host package that executes scenarios and records the observation.
const RUNNER: &str = "oer-hil-runner";

/// Directories read whole: the firmware workspace, the platform's linker and
/// stack inputs, the scenario catalog and Cargo's configuration.
const DIRECTORIES: [&str; 4] = [
    "hil/targets/esp32s31",
    "platform/esp32s31",
    "hil/scenarios",
    ".cargo",
];

/// Workspace files that configure every build.
const FILES: [&str; 4] = [
    "Cargo.toml",
    "Cargo.lock",
    "rust-toolchain.toml",
    "rust-toolchain",
];

pub(super) struct Closure {
    directories: BTreeSet<PathBuf>,
}

impl Closure {
    /// The closure of the checkout at `root`, from Cargo's locked metadata.
    pub(super) fn of(root: &Path) -> Result<Self> {
        let root = root.canonicalize()?;
        let mut directories = DIRECTORIES
            .map(PathBuf::from)
            .into_iter()
            .collect::<BTreeSet<_>>();
        for workspace in FIRMWARE_WORKSPACES {
            let metadata = metadata(&root, &root.join(workspace))?;
            for package in metadata["packages"].as_array().into_iter().flatten() {
                if package["source"].is_null()
                    && let Some(directory) = package_directory(&root, package)
                {
                    directories.insert(directory);
                }
            }
        }
        directories.extend(runner_packages(
            &root,
            &metadata(&root, &root.join("Cargo.toml"))?,
        )?);
        Ok(Self { directories })
    }

    pub(super) fn contains(&self, path: &Path) -> bool {
        FILES.iter().any(|file| path == Path::new(file))
            || self
                .directories
                .iter()
                .any(|directory| path.starts_with(directory))
    }

    #[cfg(test)]
    pub(super) fn from_directories(directories: &[&str]) -> Self {
        Self {
            directories: directories.iter().map(PathBuf::from).collect(),
        }
    }
}

fn metadata(root: &Path, manifest: &Path) -> Result<Value> {
    let output = Command::new(std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into()))
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--offline", "--locked"])
        .arg("--manifest-path")
        .arg(manifest)
        .output()?;
    if !output.status.success() {
        return Err(format!(
            "cannot list the packages of {}: {}",
            manifest.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )
        .into());
    }
    Ok(serde_json::from_slice(&output.stdout)?)
}

/// The repository-relative directory of a package inside `root`.
fn package_directory(root: &Path, package: &Value) -> Option<PathBuf> {
    let manifest = Path::new(package["manifest_path"].as_str()?);
    let directory = manifest.parent()?.canonicalize().ok()?;
    directory.strip_prefix(root).ok().map(Path::to_owned)
}

/// Directories of the path packages the runner depends on, transitively.
fn runner_packages(root: &Path, metadata: &Value) -> Result<BTreeSet<PathBuf>> {
    let packages = metadata["packages"]
        .as_array()
        .ok_or("workspace metadata lists no packages")?;
    let by_id = packages
        .iter()
        .filter_map(|package| Some((package["id"].as_str()?, package)))
        .collect::<BTreeMap<_, _>>();
    let runner = packages
        .iter()
        .find(|package| package["name"] == RUNNER && package["source"].is_null())
        .and_then(|package| package["id"].as_str())
        .ok_or("the workspace has no HIL runner package")?;
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("workspace metadata has no dependency graph")?
        .iter()
        .filter_map(|node| Some((node["id"].as_str()?, node)))
        .collect::<BTreeMap<_, _>>();
    let mut pending = vec![runner];
    let mut seen = BTreeSet::new();
    let mut directories = BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id) {
            continue;
        }
        let Some(package) = by_id.get(id) else {
            continue;
        };
        if !package["source"].is_null() {
            continue;
        }
        if let Some(directory) = package_directory(root, package) {
            directories.insert(directory);
        }
        for dependency in nodes
            .get(id)
            .and_then(|node| node["deps"].as_array())
            .into_iter()
            .flatten()
        {
            if let Some(id) = dependency["pkg"].as_str() {
                pending.push(id);
            }
        }
    }
    Ok(directories)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn only_build_and_run_inputs_are_in_the_closure() {
        let closure = Closure::from_directories(&["crates/hardware/esp32s31/hal", "hil/scenarios"]);
        assert!(closure.contains(Path::new("crates/hardware/esp32s31/hal/src/lib.rs")));
        assert!(closure.contains(Path::new("hil/scenarios/system/boot-smoke.toml")));
        assert!(closure.contains(Path::new("Cargo.lock")));
        assert!(!closure.contains(Path::new("docs/architecture.md")));
        assert!(!closure.contains(Path::new("crates/hardware/esp32s31/hal-extra/src/lib.rs")));
        assert!(!closure.contains(Path::new("tools/blobray/src/main.rs")));
    }

    #[test]
    fn the_runner_closure_follows_path_dependencies_only() {
        let root = std::env::temp_dir().join(format!("closure-{}", std::process::id()));
        for directory in ["hil/host/runner", "hil/host/core", "tools/other"] {
            fs::create_dir_all(root.join(directory)).unwrap();
        }
        let root = root.canonicalize().unwrap();
        let manifest = |directory: &str| root.join(directory).join("Cargo.toml");
        let metadata = json!({
            "packages": [
                {"id": "runner", "name": RUNNER, "source": null,
                 "manifest_path": manifest("hil/host/runner")},
                {"id": "core", "name": "core", "source": null,
                 "manifest_path": manifest("hil/host/core")},
                {"id": "other", "name": "other", "source": null,
                 "manifest_path": manifest("tools/other")},
                {"id": "serde", "name": "serde", "source": "registry+https://github.com/rust-lang/crates.io-index",
                 "manifest_path": "/registry/serde/Cargo.toml"},
            ],
            "resolve": {"nodes": [
                {"id": "runner", "deps": [{"pkg": "core"}, {"pkg": "serde"}]},
                {"id": "core", "deps": [{"pkg": "serde"}]},
                {"id": "other", "deps": []},
                {"id": "serde", "deps": []},
            ]},
        });
        assert_eq!(
            runner_packages(&root, &metadata).unwrap(),
            BTreeSet::from([
                PathBuf::from("hil/host/core"),
                PathBuf::from("hil/host/runner")
            ])
        );
    }
}
