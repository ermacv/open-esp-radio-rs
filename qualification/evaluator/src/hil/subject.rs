//! Identity of an observed execution, independent of current applicability.
//!
//! All paths refer to material already covered by the completion seal. Older
//! bundles retain absent identities explicitly; a commit or build label never
//! substitutes for the bytes of an application or an experiment procedure.

use super::*;
use serde::Serialize;

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct FileIdentity {
    pub(super) path: PathBuf,
    pub(super) size_bytes: u64,
    pub(super) sha256: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct FirmwareIdentity {
    pub(super) image: Option<String>,
    pub(super) build_id: Option<String>,
    pub(super) application: Option<FileIdentity>,
    pub(super) build_provenance: Option<FileIdentity>,
    pub(super) replayed: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(super) struct ObservationSubject {
    pub(super) repository: RepositoryProvenance,
    /// Images recorded in this completion boundary; no missing association is inferred.
    pub(super) firmware: Vec<FirmwareIdentity>,
    pub(super) procedure: Option<FileIdentity>,
    fixture: Option<FileIdentity>,
    pub(super) source_snapshot_manifest: Option<FileIdentity>,
}

impl ObservationSubject {
    pub(super) fn load(run: &Path, manifest: &RunManifest, scenario: &str) -> Result<Self> {
        let mut firmware = Vec::new();
        for artifact in &manifest.firmware {
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
            repository: manifest.repository.clone(),
            firmware,
            procedure: file(
                run,
                &PathBuf::from("scenarios")
                    .join(scenario)
                    .join("scenario.json"),
            )?,
            fixture: file(run, Path::new("lab-provenance.json"))?,
            source_snapshot_manifest: file(run, Path::new("source/snapshot/manifest.json"))?,
        })
    }
}

fn file(run: &Path, relative: &Path) -> Result<Option<FileIdentity>> {
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
