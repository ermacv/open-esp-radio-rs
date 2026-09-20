//! Qualification accepts only reconstructable sources from the pinned composition.
//!
//! Diagnostic overrides remain valid run bundles, but a clean main checkout alone
//! does not establish the identity of their firmware inputs.

use super::snapshot::Source as SourceMaterial;
use super::*;

#[derive(Deserialize)]
struct BuildProvenance {
    schema: u16,
    build_id: String,
    build_type: String,
    source_reconstructable: bool,
    sources: Vec<SourceMaterial>,
    files: Vec<FileMaterial>,
    parameters: Option<serde_json::Value>,
    #[serde(default)]
    subjects: Vec<BuildSubject>,
}

#[derive(Deserialize)]
struct BuildSubject {
    role: String,
    size_bytes: u64,
    sha256: String,
}

fn application_bound(provenance: &BuildProvenance, artifact: &FirmwareArtifactProvenance) -> bool {
    let Some(image) = artifact.image.as_deref() else {
        return false;
    };
    if provenance
        .parameters
        .as_ref()
        .and_then(|p| p.get("image"))
        .and_then(serde_json::Value::as_str)
        != Some(image)
    {
        return false;
    }
    let applications = provenance
        .subjects
        .iter()
        .filter(|s| s.role == "application")
        .collect::<Vec<_>>();
    let [application] = applications.as_slice() else {
        return false;
    };
    artifact.application_path.is_some()
        && artifact.application_size_bytes == Some(application.size_bytes)
        && artifact.application_sha256.as_ref() == Some(&application.sha256)
}

#[derive(Deserialize)]
struct FileMaterial {
    name: String,
    path: PathBuf,
    sha256: String,
}

#[derive(Deserialize)]
struct CargoLock {
    package: Vec<LockedPackage>,
}

#[derive(Deserialize)]
struct LockedPackage {
    name: String,
    source: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
pub(super) enum Binding {
    Unavailable,
    Commit,
    Snapshot,
}

pub(super) fn current_sources(root: &Path, run: &Path, manifest: &RunManifest) -> Result<Binding> {
    let mut binding = Binding::Snapshot;
    if manifest.firmware.is_empty() {
        return Ok(Binding::Unavailable);
    }
    for artifact in &manifest.firmware {
        let (Some(build_id), Some(path)) = (&artifact.build_id, &artifact.build_provenance_path)
        else {
            // Older diagnostic bundles can lack build provenance; they cannot
            // establish the source composition required for qualification.
            return Ok(Binding::Unavailable);
        };
        if !safe_relative(path) {
            return Err("HIL build provenance path must be contained in the run bundle".into());
        }
        let provenance: BuildProvenance = read_json(&run.join(path))?;
        if provenance.schema != 1
            || provenance.build_type != "open-esp-radio-hil-firmware/v1"
            || &provenance.build_id != build_id
            || !valid_sha256(build_id)
        {
            return Err("HIL build provenance identity is inconsistent with its artifact".into());
        }
        if !provenance.source_reconstructable {
            return Ok(Binding::Unavailable);
        }
        let Some(primary) = provenance.sources.first() else {
            return Ok(Binding::Unavailable);
        };
        if primary.name != "repository"
            || primary.commit != manifest.repository.commit
            || primary.workspace_sha256 != manifest.repository.workspace_sha256
        {
            return Ok(Binding::Unavailable);
        }
        let mut names = BTreeSet::new();
        for source in &provenance.sources {
            if !names.insert(&source.name)
                || (source.dirty
                    && (source.rebuild_status != "source-snapshot" || source.name != "repository"))
                || !matches!(
                    source.rebuild_status.as_str(),
                    "clean-commit" | "source-snapshot"
                )
                || !valid_sha256(&source.workspace_sha256)
                || !source.limitations.is_empty()
                || !source.untracked_files.is_empty()
                || source.tracked_patch_path.is_some()
            {
                return Ok(Binding::Unavailable);
            }
        }
        if provenance
            .sources
            .iter()
            .any(|s| s.rebuild_status == "source-snapshot")
            && (!provenance
                .sources
                .iter()
                .all(|s| s.rebuild_status == "source-snapshot")
                || !application_bound(&provenance, artifact)
                || !snapshot::current(root, run, &provenance.sources)?)
        {
            return Ok(Binding::Unavailable);
        }
        if primary.rebuild_status != "source-snapshot" {
            binding = Binding::Commit;
        }
        // Bind the pin authority to this checkout, rather than trusting the
        // versions claimed by an override's effective (path-patched) lockfile.
        let locks = provenance
            .files
            .iter()
            .filter(|file| file.name == "workspace-lock")
            .collect::<Vec<_>>();
        let [lock] = locks.as_slice() else {
            return Ok(Binding::Unavailable);
        };
        let lock_path = root.join("Cargo.lock");
        if lock.path != Path::new("Cargo.lock") || lock.sha256 != sha256_file(&lock_path)? {
            return Ok(Binding::Unavailable);
        }
        if provenance.sources.len() > 1 {
            let lock: CargoLock = toml_edit::de::from_str(&fs::read_to_string(lock_path)?)?;
            for source in &provenance.sources[1..] {
                if !matches_pin(source, &lock) {
                    return Ok(Binding::Unavailable);
                }
            }
        }
    }
    Ok(binding)
}

fn matches_pin(source: &SourceMaterial, lock: &CargoLock) -> bool {
    // Names are the serialized source roles, not local checkout paths. Require
    // every package patched by each supported override to share its pinned rev.
    let packages: &[&str] = match source.name.as_str() {
        "esp-hal" => &["esp-hal", "esp-sync", "esp-bootloader-esp-idf"],
        "embassy" => &["embassy-net", "embassy-net-driver"],
        "xarxa" => &["xarxa-driver"],
        _ => return false,
    };
    packages.iter().all(|name| {
        let sources = lock
            .package
            .iter()
            .filter(|package| package.name == *name)
            .filter_map(|package| package.source.as_deref())
            .filter(|source| source.starts_with("git+"))
            .collect::<Vec<_>>();
        !sources.is_empty()
            && sources.iter().all(|pin| {
                pin.rsplit_once('#')
                    .is_some_and(|(_, commit)| commit == source.commit)
            })
    })
}
