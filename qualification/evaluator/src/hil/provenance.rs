//! Qualification accepts only reconstructable sources from the pinned composition.
//!
//! Diagnostic overrides remain valid run bundles, but a clean main checkout alone
//! does not establish the identity of their firmware inputs.

use super::*;

#[derive(Deserialize)]
struct BuildProvenance {
    schema: u16,
    build_id: String,
    build_type: String,
    source_reconstructable: bool,
    sources: Vec<SourceMaterial>,
    files: Vec<FileMaterial>,
}

#[derive(Deserialize)]
struct SourceMaterial {
    name: String,
    commit: String,
    dirty: bool,
    workspace_sha256: String,
    rebuild_status: String,
    limitations: Vec<serde::de::IgnoredAny>,
    untracked_files: Vec<serde::de::IgnoredAny>,
    tracked_patch_path: Option<PathBuf>,
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

pub(super) fn current_sources(root: &Path, run: &Path, manifest: &RunManifest) -> Result<bool> {
    if manifest.firmware.is_empty() {
        return Ok(false);
    }
    for artifact in &manifest.firmware {
        let (Some(build_id), Some(path)) = (&artifact.build_id, &artifact.build_provenance_path)
        else {
            // Older diagnostic bundles can lack build provenance; they cannot
            // establish the source composition required for qualification.
            return Ok(false);
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
            return Ok(false);
        }
        let Some(primary) = provenance.sources.first() else {
            return Ok(false);
        };
        if primary.name != "repository"
            || primary.commit != manifest.repository.commit
            || primary.workspace_sha256 != manifest.repository.workspace_sha256
        {
            return Ok(false);
        }
        let mut names = BTreeSet::new();
        for source in &provenance.sources {
            if !names.insert(&source.name)
                || source.dirty
                || source.rebuild_status != "clean-commit"
                || !valid_sha256(&source.workspace_sha256)
                || !source.limitations.is_empty()
                || !source.untracked_files.is_empty()
                || source.tracked_patch_path.is_some()
            {
                return Ok(false);
            }
        }
        // Bind the pin authority to this checkout, rather than trusting the
        // versions claimed by an override's effective (path-patched) lockfile.
        let locks = provenance
            .files
            .iter()
            .filter(|file| file.name == "workspace-lock")
            .collect::<Vec<_>>();
        let [lock] = locks.as_slice() else {
            return Ok(false);
        };
        let lock_path = root.join("Cargo.lock");
        if lock.path != Path::new("Cargo.lock") || lock.sha256 != sha256_file(&lock_path)? {
            return Ok(false);
        }
        if provenance.sources.len() > 1 {
            let lock: CargoLock = toml_edit::de::from_str(&fs::read_to_string(lock_path)?)?;
            for source in &provenance.sources[1..] {
                if !matches_pin(source, &lock) {
                    return Ok(false);
                }
            }
        }
    }
    Ok(true)
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
