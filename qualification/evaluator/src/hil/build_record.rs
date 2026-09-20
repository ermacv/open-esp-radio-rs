//! Independent build subjects; never inserted into the scenario evidence index.
use super::*;

pub(super) struct BuildEvidence {
    pub directory: PathBuf,
    pub subject: subject::ObservationSubject,
}
impl BuildEvidence {
    pub(super) fn observation(observation: &ScenarioEvidence) -> Option<Self> {
        Some(Self {
            directory: observation.run_directory.clone()?,
            subject: observation.subject.clone()?,
        })
    }
    pub(super) fn load(root: &Path, path: &Path, identity: &str) -> Result<Self> {
        if !safe_relative(path) {
            return Err("build record must be repository-relative".into());
        }
        let seal = subject::file(root, &path.join("integrity.json"))?
            .ok_or("build record has no completion seal")?;
        if seal.sha256 != identity {
            return Err("build record seal does not match the reviewed identity".into());
        }
        let directory = root.join(path);
        verify_integrity_named(&directory, "build-only")?;
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Record {
            schema: u16,
            kind: String,
            target: String,
            repository: RepositoryProvenance,
            firmware: Vec<FirmwareArtifactProvenance>,
        }
        let record: Record = read_json(&directory.join("build.json"))?;
        if record.schema != 1
            || record.kind != "open-esp-radio-build"
            || record.target != "esp32s31"
            || record.firmware.len() != 1
            || !valid_sha256(&record.repository.workspace_sha256)
        {
            return Err("invalid build-only subject".into());
        }
        let subject = subject::ObservationSubject::from_parts(
            &directory,
            &record.repository,
            &record.firmware,
            None,
        )?;
        Ok(Self { directory, subject })
    }
}
