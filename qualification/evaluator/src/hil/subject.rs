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
    /// The runner's observer record, naming its build by digest; the build
    /// is read only while a decision needs it.
    pub(super) observer: Option<ObserverReference>,
    pub(super) repository: RepositoryProvenance,
    /// Images recorded in this completion boundary; no missing association is inferred.
    pub(super) firmware: Vec<FirmwareIdentity>,
    pub(super) procedure: Option<FileIdentity>,
    fixture: Option<FileIdentity>,
    pub(super) source_snapshot_manifest: Option<FileIdentity>,
}

/// A stored observer record: a reference naming the observer's build by
/// digest, and the store holding that build.
///
/// The build carries the observer's resolved Cargo graph unrolled into a
/// tree, hundreds of megabytes of JSON and several times that once parsed,
/// and the store keeps one per observer the runs ever used. An observation
/// therefore holds only this reference: the build is checked to be present
/// and to hash to its name when the reference is resolved, and parsed by
/// [`Self::proof`] only while a decision needs it, never kept.
#[derive(Clone, Debug)]
pub(super) struct ObserverReference {
    record: serde_json::Value,
    store: PathBuf,
}

/// The identity is the record; the store is only where its build was found.
impl PartialEq for ObserverReference {
    fn eq(&self, other: &Self) -> bool {
        self.record == other.record
    }
}

impl Eq for ObserverReference {}

impl Serialize for ObserverReference {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        self.record.serialize(serializer)
    }
}

/// A read reference names no store yet: it is resolved against its own with
/// [`ObservationSubject::resolve_observer`] before use.
impl<'de> Deserialize<'de> for ObserverReference {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        Ok(Self {
            record: serde_json::Value::deserialize(deserializer)?,
            store: PathBuf::new(),
        })
    }
}

impl ObserverReference {
    /// The stored `record` whose build lives in `store`. The build must be
    /// present there and hash to its name, else resolving fails.
    pub(super) fn resolve(record: serde_json::Value, store: &Path) -> Result<Self> {
        use oer_hil_run_bundle_format::observer::store::{self as observer_store, REFERENCED};
        if record["schema"] != REFERENCED {
            return Err(format!(
                "HIL observer proof has schema {}; only references (schema {REFERENCED}) are read",
                record["schema"]
            )
            .into());
        }
        let digest = observer_store::build_digest(&record).unwrap_or_default();
        let path = observer_store::path(store, digest)
            .map_err(|error| format!("HIL observer proof: {error}"))?;
        let sha256 = crate::digests().sha256_file(&path).map_err(|error| {
            format!(
                "HIL observer proof: build {} is missing: {error}",
                path.display()
            )
        })?;
        if sha256 != digest {
            return Err(format!(
                "HIL observer proof: build {} does not hash to its name",
                path.display()
            )
            .into());
        }
        Ok(Self {
            record,
            store: store.to_owned(),
        })
    }

    /// The build and executable the record names, which identify the
    /// observer.
    pub(super) fn identity(&self) -> (String, String) {
        (
            self.record["build_sha256"].to_string(),
            self.record["executable_sha256"].to_string(),
        )
    }

    /// The record with its build inline, read from the store; the caller
    /// holds it only while it needs it.
    pub(super) fn proof(&self) -> Result<serde_json::Value> {
        #[cfg(test)]
        super::observer::work::count(|work| work.build_loads += 1);
        oer_hil_run_bundle_format::observer::store::attach(&self.record, &self.store)
            .map_err(|error| format!("HIL observer proof: {error}").into())
    }
}

impl ObservationSubject {
    pub(super) fn load(run: &Path, manifest: &RunManifest, scenario: &str) -> Result<Self> {
        let mut subject = Self::from_parts(
            run,
            &manifest.repository,
            &manifest.firmware,
            Some(scenario),
        )?;
        // The store holding the run's referenced observer build is beside the
        // directory the run's directory resolves into.
        let store = fs::canonicalize(run)?
            .parent()
            .and_then(Path::parent)
            .ok_or("HIL run directory has no observer store")?
            .to_owned();
        subject.observer = manifest
            .observer()
            .map(|record| ObserverReference::resolve(record.clone(), &store))
            .transpose()?;
        Ok(subject)
    }

    /// Resolve the observer reference against `store`, which must hold its
    /// build.
    pub(super) fn resolve_observer(&mut self, store: &Path) -> Result<()> {
        if let Some(observer) = &mut self.observer {
            *observer = ObserverReference::resolve(observer.record.clone(), store)?;
        }
        Ok(())
    }
    pub(super) fn from_parts(
        run: &Path,
        repository: &RepositoryProvenance,
        artifacts: &[FirmwareArtifact],
        scenario: Option<&str>,
    ) -> Result<Self> {
        let mut firmware = Vec::new();
        for artifact in artifacts {
            if artifact
                .build_id
                .as_ref()
                .is_some_and(|id| !valid_sha256(id))
            {
                return Err("HIL firmware subject has an invalid image or build identity".into());
            }
            let application = file(run, &artifact.application_path)?
                .ok_or("HIL application subject is missing")?;
            if application.size_bytes != artifact.application_size_bytes
                || application.sha256 != artifact.application_sha256
            {
                return Err("HIL application subject disagrees with its archived bytes".into());
            }
            let application = Some(application);
            let build_provenance = artifact
                .build_provenance_path
                .as_ref()
                .map(|path| -> Result<FileIdentity> {
                    file(run, path)?.ok_or_else(|| "HIL build provenance subject is missing".into())
                })
                .transpose()?;
            firmware.push(FirmwareIdentity {
                image: Some(artifact.image.id().to_owned()),
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
        sha256: crate::digests()
            .sha256_file(&path)
            .map_err(|error| error.to_string())?,
    }))
}
