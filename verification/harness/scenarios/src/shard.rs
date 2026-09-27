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
    let mut directories = path_closure(&root, probes.manifest, package, Some(probes.target))?;
    directories.extend(path_closure(&root, TOOL_MANIFEST, tool_package, None)?);
    directories.push(PathBuf::from(SCHEMA_SOURCES));
    directories.sort();
    directories.dedup();
    let sources = directories
        .into_iter()
        .map(|path| {
            Ok(evidence_index::SourceDigest {
                sha256: evidence_index::digest_directory(&root, &path)?,
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
    let index = Index {
        schema: evidence_index::SCHEMA,
        command: evidence_index::COMMAND.into(),
        target: target.into(),
        scenario: scenario.into(),
        inputs: claims.inputs.clone(),
        sources,
        entries: claims.entries.clone(),
        untriaged: coverage::uncovered_everywhere(&claims.closures, claims.untriaged.clone())
            .into_iter()
            .collect(),
        functions: claims
            .closures
            .iter()
            .flat_map(|c| c.functions.iter().cloned())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
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
