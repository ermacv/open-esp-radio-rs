//! Identity of an observed execution, independent of current applicability.
//!
//! All paths refer to material already covered by the completion seal. Older
//! bundles retain absent identities explicitly; a commit or build label never
//! substitutes for the bytes of an application or an experiment procedure.

use super::*;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct FileIdentity {
    pub(super) path: PathBuf,
    pub(super) size_bytes: u64,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct FirmwareIdentity {
    pub(super) image: Option<String>,
    pub(super) build_id: Option<String>,
    pub(super) application: Option<FileIdentity>,
    pub(super) build_provenance: Option<FileIdentity>,
    pub(super) replayed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(super) struct ObservationSubject {
    /// The runner's observer proof with its build inline, shared between
    /// observations of one observer: the build is several megabytes and most
    /// runs share one. It is written as a reference to the build's digest;
    /// a read reference is resolved against its observer store with
    /// [`ObservationSubject::resolve_observer`].
    #[serde(serialize_with = "observer_reference")]
    pub(super) observer: Option<std::sync::Arc<serde_json::Value>>,
    pub(super) repository: RepositoryProvenance,
    /// Images recorded in this completion boundary; no missing association is inferred.
    pub(super) firmware: Vec<FirmwareIdentity>,
    pub(super) procedure: Option<FileIdentity>,
    fixture: Option<FileIdentity>,
    pub(super) source_snapshot_manifest: Option<FileIdentity>,
}

fn observer_reference<S: serde::Serializer>(
    observer: &Option<std::sync::Arc<serde_json::Value>>,
    serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
    use oer_hil_schema::observer_store::{EMBEDDED, REFERENCED};
    match observer.as_deref() {
        Some(record) if record["schema"] == EMBEDDED => serde_json::json!({
            "schema": REFERENCED,
            "executable_sha256": record["executable_sha256"],
            "build_sha256": record["build_sha256"],
        })
        .serialize(serializer),
        record => record.serialize(serializer),
    }
}

/// One shared copy of each distinct observer proof, with its build inline.
/// An embedded build is hashed once per digest it claims. A referenced build
/// must be present and hash to its name, else loading fails; it is read again
/// only when its file changed since it was verified.
fn shared_observer(
    record: &serde_json::Value,
    store: &Path,
) -> Result<std::sync::Arc<serde_json::Value>> {
    use oer_hil_schema::observer_store::{self, EMBEDDED};
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex, OnceLock},
    };
    type File = (PathBuf, u64, std::time::SystemTime);
    type Key = (Option<File>, String, String);
    static PROOFS: OnceLock<Mutex<HashMap<Key, Arc<serde_json::Value>>>> = OnceLock::new();
    let digest = observer_store::build_digest(record).unwrap_or_default();
    let file = if record["schema"] == EMBEDDED {
        None
    } else {
        let path = observer_store::path(store, digest)
            .map_err(|error| format!("HIL observer proof: {error}"))?;
        let metadata = fs::metadata(&path).map_err(|error| {
            format!(
                "HIL observer proof: build {} is missing: {error}",
                path.display()
            )
        })?;
        Some((path, metadata.len(), metadata.modified()?))
    };
    let key = (
        file,
        digest.to_owned(),
        record["executable_sha256"].to_string(),
    );
    if let Some(shared) = PROOFS
        .get_or_init(Default::default)
        .lock()
        .map_err(|_| "observer proof cache poisoned")?
        .get(&key)
    {
        // An embedded record claiming a cached digest shares it only when
        // its build is the one cached.
        if record["schema"] != EMBEDDED || shared["build"] == record["build"] {
            return Ok(shared.clone());
        }
    }
    let proof = Arc::new(
        observer_store::attach(record, store)
            .map_err(|error| format!("HIL observer proof: {error}"))?,
    );
    // An embedded build whose bytes do not hash to its claim is kept apart;
    // the observer review reports its identity as differing.
    if Some(observer_store::digest(&serde_json::to_vec(&proof["build"])?).as_str())
        == observer_store::build_digest(&proof)
    {
        PROOFS
            .get_or_init(Default::default)
            .lock()
            .map_err(|_| "observer proof cache poisoned")?
            .insert(key, proof.clone());
    }
    Ok(proof)
}

impl ObservationSubject {
    pub(super) fn load(run: &Path, manifest: &RunManifest, scenario: &str) -> Result<Self> {
        let mut subject = Self::from_parts(
            run,
            &manifest.repository,
            &manifest.firmware,
            Some(scenario),
        )?;
        subject.observer = manifest
            .runner
            .as_ref()
            .and_then(|r| r.get("observer"))
            .filter(|v| !v.is_null())
            .cloned()
            .map(std::sync::Arc::new);
        // The store holding the run's referenced observer build is beside the
        // directory the run's directory resolves into.
        let store = fs::canonicalize(run)?
            .parent()
            .and_then(Path::parent)
            .ok_or("HIL run directory has no observer store")?
            .to_owned();
        subject.resolve_observer(&store)?;
        Ok(subject)
    }

    /// Replace the observer record with the shared proof carrying its build,
    /// reading a referenced build from `store`.
    pub(super) fn resolve_observer(&mut self, store: &Path) -> Result<()> {
        if let Some(record) = &self.observer {
            self.observer = Some(shared_observer(record, store)?);
        }
        Ok(())
    }
    pub(super) fn from_parts(
        run: &Path,
        repository: &RepositoryProvenance,
        artifacts: &[FirmwareArtifactProvenance],
        scenario: Option<&str>,
    ) -> Result<Self> {
        let mut firmware = Vec::new();
        for artifact in artifacts {
            if artifact.image.as_ref().is_some_and(|id| !valid_id(id))
                || artifact
                    .build_id
                    .as_ref()
                    .is_some_and(|id| !valid_sha256(id))
            {
                return Err("HIL firmware subject has an invalid image or build identity".into());
            }
            let application = match (
                &artifact.application_path,
                artifact.application_size_bytes,
                &artifact.application_sha256,
            ) {
                (None, None, None) => None,
                (Some(path), Some(size), Some(hash)) => {
                    let identity = file(run, path)?.ok_or("HIL application subject is missing")?;
                    if identity.size_bytes != size || &identity.sha256 != hash {
                        return Err(
                            "HIL application subject disagrees with its archived bytes".into()
                        );
                    }
                    Some(identity)
                }
                _ => return Err("HIL application subject identity is incomplete".into()),
            };
            let build_provenance = artifact
                .build_provenance_path
                .as_ref()
                .map(|path| -> Result<FileIdentity> {
                    file(run, path)?.ok_or_else(|| "HIL build provenance subject is missing".into())
                })
                .transpose()?;
            firmware.push(FirmwareIdentity {
                image: artifact.image.clone(),
                build_id: artifact.build_id.clone(),
                application,
                build_provenance,
                replayed: artifact.replayed_from.is_some(),
            });
        }
        Ok(Self {
            observer: None,
            repository: repository.clone(),
            firmware,
            procedure: scenario
                .map(|scenario| {
                    file(
                        run,
                        &PathBuf::from("scenarios")
                            .join(scenario)
                            .join("scenario.json"),
                    )
                })
                .transpose()?
                .flatten(),
            fixture: file(run, Path::new("lab-provenance.json"))?,
            source_snapshot_manifest: file(run, Path::new("source/snapshot/manifest.json"))?,
        })
    }
}

pub(super) fn file(run: &Path, relative: &Path) -> Result<Option<FileIdentity>> {
    if !safe_relative(relative) {
        return Err("HIL subject path must be contained in its run".into());
    }
    let mut path = run.to_owned();
    let mut components = relative.components().peekable();
    while let Some(component) = components.next() {
        path.push(component);
        let metadata = match fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if (components.peek().is_some() && !metadata.file_type().is_dir())
            || (components.peek().is_none() && !metadata.file_type().is_file())
        {
            return Err("HIL subject contains a symlink or special file".into());
        }
    }
    Ok(Some(FileIdentity {
        path: relative.to_owned(),
        size_bytes: fs::metadata(&path)?.len(),
        sha256: sha256_file(&path)?,
    }))
}
