//! Evidence shards: one scenario's claims with the digests of every source
//! its verdicts depend on, written to the scenario package's evidence index.
use crate::harness::Result;
use crate::session::{self, evidence_index};
use crate::{coverage, observation, state};
use evidence_index::Index;
use std::path::{Path, PathBuf};

/// The compiled production images a chip's scenarios run, and where their
/// sources are resolved.
pub struct ProbeImages {
    /// Probe workspace manifest, relative to the repository root.
    pub manifest: &'static str,
    /// Probe ELF packages whose images the shards may name.
    pub packages: &'static [&'static str],
    /// Target triple the probes are built for.
    pub target: &'static str,
}

/// Blobray workspace, which also resolves the scenario packages and the
/// engine behind every verdict.
const TOOL_MANIFEST: &str = "tools/blobray/Cargo.toml";
/// Shared schema sources the scenarios include by path.
const SCHEMA_SOURCES: &str = "verification/schema";
/// Production crates, which shards track by executed file.
const PRODUCTION_CRATES: &str = "crates";
/// The pinned toolchain, and the lock file and package manifest names.
const TOOLCHAIN: &str = "rust-toolchain.toml";
const LOCK_FILE: &str = "Cargo.lock";
const PACKAGE_MANIFEST: &str = "Cargo.toml";
/// The probe compiler, a host build tool whose sources Cargo's dep-info of
/// the probe image does not list.
const PROBE_COMPILER: [&str; 2] = [
    "verification/harness/codegen",
    "verification/harness/macros",
];
/// Directory of a probe workspace's image packages: their placement and
/// entry code apply to every probe, so they stay global.
const PROBE_IMAGE_DIRECTORY: &str = "elf";

/// Repository-relative sources Cargo's dep-info records for the artifact at
/// `artifact`, from the `.d` file beside it: every compiled source file,
/// `include!`/`include_str!`/`include_bytes!` input and build-script
/// `rerun-if-changed` path. Files outside the repository are pinned by the
/// lock files and the toolchain.
pub fn dep_info(root: &Path, artifact: &Path) -> Result<Vec<PathBuf>> {
    let mut path = artifact.as_os_str().to_owned();
    path.push(".d");
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "{}: {e}; build the artifact first",
            Path::new(&path).display()
        )
    })?;
    let first = text.lines().next().unwrap_or_default();
    let list = first.split_once(": ").map_or("", |(_, list)| list);
    let mut files = std::collections::BTreeSet::new();
    let mut current = String::new();
    let mut escaped = false;
    for c in list.chars().chain([' ']) {
        match (escaped, c) {
            (false, '\\') => escaped = true,
            (false, ' ') => {
                if !current.is_empty() {
                    let file = PathBuf::from(std::mem::take(&mut current));
                    if let Ok(canonical) = file.canonicalize()
                        && let Ok(relative) = canonical.strip_prefix(root)
                    {
                        files.insert(relative.to_path_buf());
                    }
                }
            }
            (_, c) => {
                escaped = false;
                current.push(c);
            }
        }
    }
    Ok(files.into_iter().collect())
}

/// Path packages in the resolved dependency closure of `package`.
fn path_closure(
    root: &Path,
    manifest: &str,
    package: &str,
    platform: Option<&str>,
) -> Result<Vec<PathBuf>> {
    let mut command = std::process::Command::new("cargo");
    command
        .current_dir(root)
        .args(["metadata", "--format-version", "1", "--offline", "--locked"])
        .args(["--manifest-path", manifest]);
    if let Some(platform) = platform {
        command.args(["--filter-platform", platform]);
    }
    let output = command.output()?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned().into());
    }
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let packages = metadata["packages"]
        .as_array()
        .ok_or("cargo metadata packages")?;
    let id_of = |name: &str| {
        packages
            .iter()
            .find(|p| p["name"] == name)
            .and_then(|p| p["id"].as_str())
            .map(str::to_owned)
    };
    let nodes = metadata["resolve"]["nodes"]
        .as_array()
        .ok_or("cargo metadata resolve")?;
    let mut pending = vec![id_of(package).ok_or_else(|| format!("package {package} missing"))?];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(id) = pending.pop() {
        if !seen.insert(id.clone()) {
            continue;
        }
        let node = nodes
            .iter()
            .find(|n| n["id"] == id.as_str())
            .ok_or("unresolved package")?;
        for dependency in node["dependencies"].as_array().into_iter().flatten() {
            pending.push(dependency.as_str().ok_or("dependency id")?.to_owned());
        }
    }
    let mut directories = std::collections::BTreeSet::new();
    for package in packages {
        let local = package["source"].is_null();
        if local && seen.contains(package["id"].as_str().unwrap_or_default()) {
            let manifest = Path::new(package["manifest_path"].as_str().ok_or("manifest path")?);
            let directory = manifest.parent().ok_or("manifest directory")?;
            directories.insert(directory.canonicalize()?.strip_prefix(root)?.to_path_buf());
        }
    }
    Ok(directories.into_iter().collect())
}

/// What Cargo compiled for one shard: the probe image's and the scenario
/// binary's dep-info, and the manifests of their path packages.
pub struct Closures {
    pub probe: Vec<PathBuf>,
    pub tool: Vec<PathBuf>,
    pub manifests: Vec<PathBuf>,
}

/// The sources a shard records, sorted: production crates and probe
/// libraries by the files the executions ran (all of them on a fallback),
/// and globally what Cargo compiled into the scenario binary, the probe
/// images' placement and entry code, every package manifest, the lock
/// files, the schema, the probe compiler and the toolchain.
pub fn source_paths(
    closures: &Closures,
    probe_manifest: &Path,
    executed: &std::collections::BTreeSet<PathBuf>,
    fallback: bool,
) -> Vec<PathBuf> {
    let probe_workspace = probe_manifest.parent().unwrap_or(Path::new(""));
    let attributable = |path: &Path| {
        path.starts_with(PRODUCTION_CRATES)
            || (path.starts_with(probe_workspace)
                && !path
                    .components()
                    .any(|c| c.as_os_str() == PROBE_IMAGE_DIRECTORY))
    };
    let (per_file, mut global): (Vec<PathBuf>, Vec<PathBuf>) = closures
        .probe
        .iter()
        .cloned()
        .partition(|path| attributable(path));
    global.extend(closures.tool.iter().cloned());
    global.extend(closures.manifests.iter().cloned());
    global.extend(PROBE_COMPILER.map(PathBuf::from));
    global.push(PathBuf::from(SCHEMA_SOURCES));
    global.push(PathBuf::from(TOOLCHAIN));
    global.push(PathBuf::from(TOOL_MANIFEST).with_file_name(LOCK_FILE));
    global.push(probe_manifest.to_path_buf());
    global.push(probe_manifest.with_file_name(LOCK_FILE));
    let production: Vec<PathBuf> = if fallback {
        per_file
    } else {
        executed
            .iter()
            .filter(|file| !global.iter().any(|g| file.starts_with(g)))
            .cloned()
            .collect()
    };
    let mut paths: Vec<PathBuf> = global.into_iter().chain(production).collect();
    paths.sort();
    paths.dedup();
    paths
}

/// The shard of `scenario`'s claims against the probe image at
/// `production`: the sources are the image package's path closure, the
/// scenario code and engine, and the shared schema.
pub fn shard(
    scenario: &str,
    production: &Path,
    claims: &session::Claims,
    probes: &ProbeImages,
    tool_package: &str,
) -> Result<Index> {
    let root = observation::root()?;
    let target = crate::chip().name;
    let package = production
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| probes.packages.contains(n))
        .ok_or_else(|| {
            format!(
                "{} is not a probe image the index knows the sources of",
                production.display()
            )
        })?;
    let mut manifests = vec![];
    for directory in path_closure(&root, probes.manifest, package, Some(probes.target))?
        .into_iter()
        .chain(path_closure(&root, TOOL_MANIFEST, tool_package, None)?)
    {
        manifests.push(directory.join(PACKAGE_MANIFEST));
    }
    let closures = Closures {
        probe: dep_info(&root, production)?,
        tool: dep_info(&root, &std::env::current_exe()?)?,
        manifests,
    };
    let dependencies = &claims.dependencies;
    let paths = source_paths(
        &closures,
        Path::new(probes.manifest),
        &dependencies.files,
        dependencies.fallback.is_some(),
    );
    let sources = paths
        .into_iter()
        .map(|path| {
            Ok(evidence_index::SourceDigest {
                sha256: evidence_index::digest_source(&root, &path)?,
                path,
            })
        })
        .collect::<Result<Vec<_>>>()?;
    let line = |(path, line): (PathBuf, u32)| evidence_index::SourceLine { path, line };
    let mut observation = observation::Sources::default();
    let (_, unobserved) =
        observation.classify(&root, crate::chip().observation, &claims.lines.unobserved())?;
    let mut unprojected = std::collections::BTreeSet::new();
    for (vendor, production, byte) in &claims.unprojected {
        let (_, untriaged) = state::classify(
            crate::chip().state,
            (vendor, production),
            &std::collections::BTreeSet::from([byte.clone()]),
        );
        unprojected.extend(untriaged);
    }
    let functions: Vec<String> = claims
        .closures
        .iter()
        .flat_map(|c| c.functions.iter().cloned())
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect();
    let decisions = PathBuf::from(crate::chip().coverage);
    let index = Index {
        schema: evidence_index::SCHEMA,
        command: evidence_index::COMMAND.into(),
        target: target.into(),
        scenario: scenario.into(),
        inputs: claims.inputs.clone(),
        sources,
        dependence: evidence_index::Dependence {
            read_data: dependencies.read_data.clone(),
            fallback: dependencies.fallback.clone(),
            coverage_decisions: Some(evidence_index::DecisionDigest {
                sha256: evidence_index::CoverageDecisions::read(&root, &decisions)?
                    .applicable_digest(&functions),
                path: decisions,
            }),
        },
        entries: claims.entries.clone(),
        untriaged: coverage::uncovered_everywhere(&claims.closures, claims.untriaged.clone())
            .into_iter()
            .collect(),
        functions,
        unobserved: unobserved.into_iter().map(line).collect(),
        observed: claims.lines.observed.iter().cloned().map(line).collect(),
        unprojected: state::ranges(&unprojected)
            .into_iter()
            .map(|(symbol, offset, length)| evidence_index::StateRange {
                symbol,
                offset,
                length,
            })
            .collect(),
    };
    index.validate(target)?;
    Ok(index)
}

/// Write `index` as its scenario's shard of the index at `directory`.
pub fn write(directory: &Path, index: &Index) -> Result<()> {
    std::fs::create_dir_all(directory)?;
    let path = directory.join(format!(
        "{}.{}",
        index.scenario,
        evidence_index::SHARD_EXTENSION
    ));
    let mut bytes = serde_json::to_vec_pretty(index)?;
    bytes.push(b'\n');
    std::fs::write(&path, bytes)?;
    println!("evidence shard {}", path.display());
    Ok(())
}

#[cfg(test)]
mod source_tests {
    use super::*;
    use std::collections::BTreeSet;

    const PROBES: &str = "verification/chip/probes/Cargo.toml";

    fn closures() -> Closures {
        Closures {
            probe: [
                "crates/hal/src/mac.rs",
                "crates/hal/src/phy.rs",
                "verification/chip/probes/radio/library/src/rx.rs",
                "verification/chip/probes/radio/library/src/i2c.rs",
                "verification/chip/probes/radio/elf/src/main.rs",
                "verification/chip/probes/radio/elf/link.x",
            ]
            .map(PathBuf::from)
            .to_vec(),
            tool: [
                "tools/blobray/crates/domain/src/lib.rs",
                "verification/chip/scenarios/src/lib.rs",
                "verification/chip/artifacts.toml",
            ]
            .map(PathBuf::from)
            .to_vec(),
            manifests: vec![PathBuf::from("crates/hal/Cargo.toml")],
        }
    }

    fn sources(executed: &[&str]) -> BTreeSet<PathBuf> {
        let executed = executed.iter().map(PathBuf::from).collect();
        source_paths(&closures(), Path::new(PROBES), &executed, false)
            .into_iter()
            .collect()
    }

    #[test]
    fn a_probe_file_is_a_source_only_of_the_shard_that_executes_it() {
        let rx = sources(&[
            "crates/hal/src/mac.rs",
            "verification/chip/probes/radio/library/src/rx.rs",
        ]);
        let i2c = sources(&[
            "crates/hal/src/phy.rs",
            "verification/chip/probes/radio/library/src/i2c.rs",
        ]);
        let edited = Path::new("verification/chip/probes/radio/library/src/rx.rs");
        assert!(rx.contains(edited));
        assert!(!i2c.contains(edited));
        assert!(!rx.contains(Path::new("crates/hal/src/phy.rs")));
    }

    #[test]
    fn a_test_or_readme_edit_is_no_shard_source() {
        // Cargo compiles neither into the scenario binary or the probe.
        for shard in [sources(&["crates/hal/src/mac.rs"]), sources(&[])] {
            assert!(!shard.contains(Path::new("tools/blobray/next/tests/execution.rs")));
            assert!(!shard.contains(Path::new("tools/blobray/README.md")));
            assert!(!shard.iter().any(|p| p == Path::new("tools/blobray")));
        }
    }

    #[test]
    fn image_placement_and_included_data_are_sources_of_every_shard() {
        for shard in [
            sources(&["crates/hal/src/mac.rs"]),
            sources(&["crates/hal/src/phy.rs"]),
        ] {
            assert!(shard.contains(Path::new("verification/chip/probes/radio/elf/link.x")));
            assert!(shard.contains(Path::new("verification/chip/probes/radio/elf/src/main.rs")));
            // An include_str!-ed data file of the scenario binary.
            assert!(shard.contains(Path::new("verification/chip/artifacts.toml")));
            assert!(shard.contains(Path::new("crates/hal/Cargo.toml")));
        }
    }

    #[test]
    fn a_fallback_tracks_every_attributable_file() {
        let shard: BTreeSet<PathBuf> =
            source_paths(&closures(), Path::new(PROBES), &BTreeSet::new(), true)
                .into_iter()
                .collect();
        assert!(shard.contains(Path::new("crates/hal/src/phy.rs")));
        assert!(shard.contains(Path::new(
            "verification/chip/probes/radio/library/src/i2c.rs"
        )));
    }

    #[test]
    fn dep_info_reads_escaped_spaces_and_keeps_repository_files() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "").unwrap();
        std::fs::write(root.join("src/a b.txt"), "").unwrap();
        let artifact = root.join("app");
        std::fs::write(
            root.join("app.d"),
            format!(
                "{}: {} {} /usr/lib/outside.rs\n\n{}:\n",
                artifact.display(),
                root.join("src/lib.rs").display(),
                root.join("src/a b.txt")
                    .display()
                    .to_string()
                    .replace(' ', "\\ "),
                root.join("src/lib.rs").display(),
            ),
        )
        .unwrap();
        assert_eq!(
            dep_info(&root, &artifact).unwrap(),
            [PathBuf::from("src/a b.txt"), PathBuf::from("src/lib.rs")]
        );
    }
}
