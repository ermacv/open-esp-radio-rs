use super::*;
use sha2::{Digest, Sha256};
use std::{
    io::{Read, Write},
    sync::Arc,
};

/// Owned, verified bytes; the parser borrows this lease and cannot reopen a source path.
#[derive(Clone)]
pub struct ArtifactLease(Arc<Vec<u8>>);

impl ArtifactLease {
    pub(crate) fn new(bytes: Vec<u8>) -> Self {
        Self(Arc::new(bytes))
    }
    pub fn bytes(&self) -> &[u8] {
        &self.0
    }
}

impl Staging {
    /// Copy one regular source file. Missing inputs are recorded; mutation and an
    /// expected-digest mismatch abort the enclosing import instead of publishing.
    pub fn capture(&mut self, source: &Path, expected: Option<&ArtifactId>) -> Result<Capture> {
        self.capture_controlled(source, expected, &mut || Ok(()))
    }

    pub fn capture_controlled(
        &mut self,
        source: &Path,
        expected: Option<&ArtifactId>,
        control: &mut dyn RunControl,
    ) -> Result<Capture> {
        control.phase(RunPhase::Capture)?;
        self.capture_with(source, expected, || {}, control)
    }

    fn capture_with(
        &mut self,
        source: &Path,
        expected: Option<&ArtifactId>,
        after_copy: impl FnOnce(),
        control: &mut dyn RunControl,
    ) -> Result<Capture> {
        let unavailable = |error: String| Capture::Unavailable {
            diagnostic: Diagnostic {
                code: DiagnosticCode::UnavailableInput,
                context: "source capture".into(),
                message: error,
            },
        };
        // Check before opening so directories and most special files are not read.
        let path_before = match fs::metadata(source) {
            Ok(metadata) => metadata,
            Err(error) => return Ok(unavailable(error.to_string())),
        };
        if !path_before.is_file() {
            return Ok(unavailable("source is not a regular file".into()));
        }
        let mut input = match File::open(source) {
            Ok(file) => file,
            Err(error) => return Ok(unavailable(error.to_string())),
        };
        let before = input.metadata().map_err(io)?;
        if !same_file(&path_before, &before) {
            return Err(Error::new(
                ErrorCode::SourceChanged,
                "source changed while opening",
            ));
        }
        let mut stage = self.disk.temporary(&self.root.join("staging"))?;
        let mut hash = Sha256::new();
        let mut length = 0u64;
        let mut buffer = [0u8; WORK_BLOCK];
        while length < before.len() {
            let count = (before.len() - length).min(WORK_BLOCK as u64) as usize;
            control.bytes(count)?;
            match input.read_exact(&mut buffer[..count]) {
                Ok(()) => (),
                Err(error) if error.kind() == std::io::ErrorKind::UnexpectedEof => {
                    return Err(Error::new(
                        ErrorCode::SourceChanged,
                        "source shortened during capture",
                    ));
                }
                Err(error) => return Ok(unavailable(error.to_string())),
            }
            control.bytes(count)?;
            stage.write_all(&buffer[..count]).map_err(io)?;
            control.bytes(count)?;
            hash.update(&buffer[..count]);
            length += count as u64;
        }
        after_copy();
        let after = input.metadata().map_err(io)?;
        let path_after = fs::metadata(source)
            .map_err(|e| Error::new(ErrorCode::SourceChanged, e.to_string()))?;
        if length != before.len() || !same_file(&before, &after) || !same_file(&before, &path_after)
        {
            return Err(Error::new(
                ErrorCode::SourceChanged,
                "source changed during capture; no revision published",
            ));
        }
        let artifact: ArtifactId = format!("{:x}", hash.finalize()).parse()?;
        if expected.is_some_and(|expected| *expected != artifact) {
            return Err(Error::new(
                ErrorCode::DigestMismatch,
                format!("source digest {artifact} differs from expected digest"),
            ));
        }
        self.persist(stage, &artifact, control)?;
        Ok(Capture::Captured { artifact, length })
    }

    pub(crate) fn persist_bytes(&mut self, bytes: &[u8]) -> Result<ArtifactId> {
        let artifact = ArtifactId::of_bytes(bytes);
        let mut stage = self.disk.temporary(&self.root.join("staging"))?;
        stage.write_all(bytes).map_err(io)?;
        self.persist(stage, &artifact, &mut || Ok(()))?;
        Ok(artifact)
    }

    pub(crate) fn persist(
        &self,
        stage: TemporaryFile,
        artifact: &ArtifactId,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        stage.as_file().sync_all().map_err(io)?;
        if stage.persist(&self.object_path(artifact))? {
            sync_dir(&self.root.join("objects"))
        } else {
            self.open_payload(artifact, control)?;
            Ok(())
        }
    }
}

impl Writer {
    pub fn capture(&mut self, source: &Path, expected: Option<&ArtifactId>) -> Result<Capture> {
        Staging {
            root: self.project.root.clone(),
            disk: TemporaryBudget::new(8 * 1024 * 1024 * 1024, None)?,
        }
        .capture(source, expected)
    }
    pub(crate) fn persist_bytes(&mut self, bytes: &[u8]) -> Result<ArtifactId> {
        Staging {
            root: self.project.root.clone(),
            disk: TemporaryBudget::new(8 * 1024 * 1024 * 1024, None)?,
        }
        .persist_bytes(bytes)
    }
    #[cfg(test)]
    fn capture_with(
        &mut self,
        source: &Path,
        expected: Option<&ArtifactId>,
        after: impl FnOnce(),
    ) -> Result<Capture> {
        Staging {
            root: self.project.root.clone(),
            disk: TemporaryBudget::new(8 * 1024 * 1024 * 1024, None)?,
        }
        .capture_with(source, expected, after, &mut || Ok(()))
    }
}

pub(crate) fn same_file(a: &fs::Metadata, b: &fs::Metadata) -> bool {
    if a.len() != b.len() || a.modified().ok() != b.modified().ok() || a.is_file() != b.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        a.dev() == b.dev()
            && a.ino() == b.ino()
            && a.ctime() == b.ctime()
            && a.ctime_nsec() == b.ctime_nsec()
    }
    #[cfg(not(unix))]
    {
        a.created().ok() == b.created().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_during_capture_never_publishes_payload_or_revision() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source");
        fs::write(&source, b"before").unwrap();
        let project = Project::create(&directory.path().join("project")).unwrap();
        let error = project
            .writer()
            .unwrap()
            .capture_with(&source, None, || {
                fs::write(&source, b"changed length").unwrap()
            })
            .unwrap_err();
        assert_eq!(error.code, ErrorCode::SourceChanged);
        assert!(project.current().unwrap().is_none());
        assert_eq!(
            fs::read_dir(project.root.join("objects")).unwrap().count(),
            0
        );
        assert_eq!(
            fs::read_dir(project.root.join("staging")).unwrap().count(),
            0
        );
    }

    #[test]
    fn writer_is_exclusive_and_released_on_drop() {
        let directory = tempfile::tempdir().unwrap();
        let project = Project::create(directory.path()).unwrap();
        let writer = project.writer().unwrap();
        assert!(matches!(
            project.writer(),
            Err(Error {
                code: ErrorCode::Busy,
                ..
            })
        ));
        drop(writer);
        assert!(project.writer().is_ok());
    }

    #[test]
    fn writer_release_is_not_delayed_by_an_inherited_descriptor() {
        let directory = tempfile::tempdir().unwrap();
        let project = Project::create(directory.path()).unwrap();
        let writer = project.writer().unwrap();
        // A concurrent spawn can duplicate open descriptors before exec closes
        // CLOEXEC files. Such a descriptor owns no writer capability.
        let inherited = writer._lock.try_clone().unwrap();
        drop(writer);
        let next = project.writer();
        assert!(
            next.is_ok(),
            "writer capability must release its lock on drop"
        );
        drop(inherited);
    }

    #[test]
    fn absent_open_has_no_side_effects_and_init_never_resets() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("absent");
        assert!(matches!(
            Project::open(&path),
            Err(Error {
                code: ErrorCode::NotFound,
                ..
            })
        ));
        assert!(!path.exists());
        let original = Project::create(&path).unwrap();
        assert!(matches!(
            Project::create(&path),
            Err(Error {
                code: ErrorCode::AlreadyExists,
                ..
            })
        ));
        assert_eq!(Project::open(&path).unwrap().id(), original.id());
    }
}
