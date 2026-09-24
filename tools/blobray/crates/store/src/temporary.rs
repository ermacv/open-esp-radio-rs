//! Owned files with pre-write capacity admission. No raw writable file escapes.
use crate::*;
use std::{
    io::{Read, Seek, SeekFrom, Write},
    sync::{Arc, Mutex},
};

#[derive(Clone)]
pub struct TemporaryBudget(Arc<Mutex<BudgetState>>);
struct BudgetState {
    usage: TemporaryUsage,
    owner: Option<RunId>,
    position: RunPosition,
}
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TemporaryConfig {
    pub schema: u32,
    pub owner: RunId,
    pub limit_bytes: u64,
}
impl TemporaryBudget {
    /// Named external output has deterministic tool-visible spelling and refunds
    /// capacity only after deletion, just like an anonymous temporary fragment.
    pub fn external(&self, path: &Path, maximum: u64) -> Result<ExternalOutput> {
        let mut file = self.create(path)?;
        file.remove = true;
        file.external(maximum)
    }
    pub fn new(limit: u64, owner: Option<RunId>) -> Result<Self> {
        if limit < TEMPORARY_CONTROL_BYTES {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "temporary limit is smaller than the 1 MiB control reserve",
            ));
        }
        Ok(Self(Arc::new(Mutex::new(BudgetState {
            usage: TemporaryUsage {
                limit_bytes: limit,
                current_bytes: TEMPORARY_CONTROL_BYTES,
                peak_bytes: TEMPORARY_CONTROL_BYTES,
                control_reserved_bytes: TEMPORARY_CONTROL_BYTES,
            },
            owner,
            position: RunPosition::default(),
        }))))
    }
    /// Low-level callers without operation metadata get the documented 8 GiB cap.
    pub fn open(stage: &Path) -> Result<Self> {
        let file = match File::open(stage.join("temporary.json")) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Self::new(8 * 1024 * 1024 * 1024, None);
            }
            Err(e) => return Err(storage_io(e)),
        };
        let mut bytes = Vec::new();
        file.take(4097)
            .read_to_end(&mut bytes)
            .map_err(storage_io)?;
        if bytes.len() > 4096 {
            return Err(integrity("temporary configuration exceeds limit"));
        }
        let config: TemporaryConfig = serde_json::from_slice(&bytes).map_err(jobs::json)?;
        if config.schema != 1 {
            return Err(Error::new(
                ErrorCode::Incompatible,
                "unsupported temporary configuration",
            ));
        }
        Self::new(config.limit_bytes, Some(config.owner))
    }
    pub fn position(&self, position: RunPosition) {
        self.0.lock().unwrap().position = position;
    }
    pub fn temporary(&self, directory: &Path) -> Result<TemporaryFile> {
        let file = tempfile::NamedTempFile::new_in(directory).map_err(storage_io)?;
        let (file, path) = file.keep().map_err(|e| storage_io(e.error))?;
        Ok(TemporaryFile {
            file,
            path,
            budget: self.clone(),
            length: 0,
            remove: true,
            refund: None,
        })
    }
    pub fn create(&self, path: &Path) -> Result<TemporaryFile> {
        let file = OpenOptions::new()
            .create_new(true)
            .read(true)
            .write(true)
            .open(path)
            .map_err(storage_io)?;
        Ok(TemporaryFile {
            file,
            path: path.into(),
            budget: self.clone(),
            length: 0,
            remove: false,
            refund: None,
        })
    }
}
impl TemporaryCapacity for TemporaryBudget {
    fn reserve(&self, bytes: u64) -> Result<()> {
        let mut state = self.0.lock().unwrap();
        let available = state.usage.limit_bytes - state.usage.current_bytes;
        if bytes > available {
            let mut error = Error::new(
                ErrorCode::ResourceLimited,
                "temporary storage budget exhausted",
            );
            error.storage = Some(Box::new(StorageFailure {
                requested_bytes: bytes,
                available_bytes: available,
                limit_bytes: state.usage.limit_bytes,
                owner: state.owner.clone(),
                position: state.position,
            }));
            return Err(error);
        }
        state.usage.current_bytes += bytes;
        state.usage.peak_bytes = state.usage.peak_bytes.max(state.usage.current_bytes);
        Ok(())
    }
    fn release(&self, bytes: u64) {
        let mut state = self.0.lock().unwrap();
        state.usage.current_bytes -= bytes;
    }
    fn usage(&self) -> TemporaryUsage {
        self.0.lock().unwrap().usage
    }
}
/// Scoped writes. A named result persists until its workspace is removed;
/// anonymous fragments refund their capacity only after successful deletion.
/// ```compile_fail
/// fn bypass(file: &mut blobray_store::TemporaryFile) { file.as_raw_file().set_len(u64::MAX); }
/// ```
pub struct TemporaryFile {
    file: File,
    path: PathBuf,
    budget: TemporaryBudget,
    length: u64,
    remove: bool,
    refund: Option<Refund>,
}
struct Refund {
    budget: TemporaryBudget,
    bytes: u64,
}
impl Drop for Refund {
    fn drop(&mut self) {
        self.budget.release(self.bytes);
    }
}
impl TemporaryFile {
    /// Reserve an external writer's maximum extent before exposing its path.
    /// The caller must enforce `maximum` in the child and reap it before finishing
    /// or dropping the lease. The reservation survives truncation by the child.
    pub fn external(mut self, maximum: u64) -> Result<ExternalOutput> {
        if self.length != 0 || maximum == 0 {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "external output needs an empty file and positive capacity",
            ));
        }
        self.budget.reserve(maximum)?;
        self.length = maximum;
        Ok(ExternalOutput {
            file: self,
            maximum,
        })
    }
    pub fn path(&self) -> &Path {
        &self.path
    }
    pub fn sync_all(&self) -> std::io::Result<()> {
        self.file.sync_all()
    }
    pub fn metadata(&self) -> std::io::Result<fs::Metadata> {
        self.file.metadata()
    }
    pub(crate) fn as_file(&self) -> &Self {
        self
    }
    pub(crate) fn as_file_mut(&mut self) -> &mut Self {
        self
    }
    pub fn set_len(&mut self, length: u64) -> std::io::Result<()> {
        let growth = length.saturating_sub(self.length);
        self.budget.reserve(growth).map_err(std::io::Error::other)?;
        if let Err(e) = self.file.set_len(length) {
            self.budget.release(growth);
            return Err(e);
        }
        self.budget.release(self.length.saturating_sub(length));
        self.length = length;
        Ok(())
    }
    /// Transfer to a staged immutable name without increasing its logical charge.
    pub(crate) fn persist(mut self, destination: &Path) -> Result<bool> {
        match fs::hard_link(&self.path, destination) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
            Err(e) => return Err(storage_io(e)),
        }
        let old = std::mem::replace(&mut self.path, destination.into());
        self.remove = false;
        fs::remove_file(old).map_err(storage_io)?;
        Ok(true)
    }
}

/// Exclusive, pre-admitted file for a bounded external process. Never clone it.
pub struct ExternalOutput {
    file: TemporaryFile,
    maximum: u64,
}
impl ExternalOutput {
    pub fn path(&self) -> &Path {
        self.file.path()
    }
    pub fn maximum(&self) -> u64 {
        self.maximum
    }
    /// Call only after all external writers have exited and been reaped.
    pub fn finish(mut self) -> Result<TemporaryFile> {
        let metadata = self.file.metadata().map_err(storage_io)?;
        let path_metadata = fs::symlink_metadata(self.file.path()).map_err(storage_io)?;
        if !metadata.is_file() || !super::capture::same_file(&metadata, &path_metadata) {
            // The owned inode may survive under another name. Do not refund or
            // delete the replacement; workspace cleanup owns the remaining charge.
            self.file.remove = false;
            return Err(Error::new(
                ErrorCode::SourceChanged,
                "external output path was replaced",
            ));
        }
        let actual = metadata.len();
        if actual > self.maximum {
            return Err(Error::new(
                ErrorCode::Integrity,
                "external output exceeded its admitted extent",
            ));
        }
        self.file.budget.release(self.maximum - actual);
        self.file.length = actual;
        Ok(self.file)
    }
}
impl TemporaryFile {
    fn write_with(
        &mut self,
        bytes: &[u8],
        write: impl FnOnce(&mut File, &[u8]) -> std::io::Result<usize>,
    ) -> std::io::Result<usize> {
        if bytes.is_empty() {
            return Ok(0);
        }
        let position = self.file.stream_position()?;
        let end = position.checked_add(bytes.len() as u64).ok_or_else(|| {
            std::io::Error::other(Error::new(
                ErrorCode::ResourceLimited,
                "temporary file offset overflow",
            ))
        })?;
        let growth = end.saturating_sub(self.length);
        self.budget.reserve(growth).map_err(std::io::Error::other)?;
        match write(&mut self.file, bytes) {
            Ok(count) => {
                let actual = if count == 0 {
                    self.length
                } else {
                    self.length.max(position + count as u64)
                };
                self.budget.release(growth - (actual - self.length));
                self.length = actual;
                Ok(count)
            }
            Err(e) => {
                self.budget.release(growth);
                Err(e)
            }
        }
    }
}
impl Write for TemporaryFile {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.write_with(bytes, |file, bytes| file.write(bytes))
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}
impl Read for TemporaryFile {
    fn read(&mut self, bytes: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(bytes)
    }
}
impl Seek for TemporaryFile {
    fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
        self.file.seek(position)
    }
}
impl Drop for TemporaryFile {
    fn drop(&mut self) {
        if self.remove && fs::remove_file(&self.path).is_ok() {
            self.refund = Some(Refund {
                budget: self.budget.clone(),
                bytes: self.length,
            });
        }
    }
}
/// Keeps the last physical context and observation in the shared run accounting.
pub struct TemporaryControl<'a> {
    pub control: &'a mut dyn RunControl,
    pub budget: &'a TemporaryBudget,
}
impl RunControl for TemporaryControl<'_> {
    fn measure(&mut self, metric: WorkMetric, amount: u64) {
        self.control.measure(metric, amount);
    }
    fn progress(&self) -> Option<RunProgress> {
        self.control.progress()
    }
    fn checkpoint(&mut self, units: u64) -> Result<()> {
        self.budget.position(self.control.position());
        self.control.temporary_storage(self.budget.usage());
        self.control.checkpoint(units)
    }
    fn position(&self) -> RunPosition {
        self.control.position()
    }
    fn set_position(&mut self, p: RunPosition) {
        self.budget.position(p);
        self.control.set_position(p)
    }
    fn memory_phases(&mut self, phases: &PhaseMeasurements) {
        self.control.memory_phases(phases);
    }
    fn working_memory(&mut self, o: WorkingMemoryObservation) {
        self.control.working_memory(o)
    }
    fn temporary_storage(&mut self, o: TemporaryUsage) {
        self.control.temporary_storage(o)
    }
}
impl Drop for TemporaryControl<'_> {
    fn drop(&mut self) {
        self.control.temporary_storage(self.budget.usage());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn growth_overwrite_sparse_truncate_and_drop_share_one_capacity() {
        let dir = tempfile::tempdir().unwrap();
        let budget = TemporaryBudget::new(TEMPORARY_CONTROL_BYTES + 10, None).unwrap();
        let mut first = budget.temporary(dir.path()).unwrap();
        let mut second = budget.temporary(dir.path()).unwrap();
        first.write_all(b"abcdef").unwrap();
        first.rewind().unwrap();
        first.write_all(b"ABC").unwrap();
        assert_eq!(budget.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 6);
        second.seek(SeekFrom::Start(3)).unwrap();
        second.write_all(b"x").unwrap();
        let error = storage_io(first.write_all(b"toolong!").unwrap_err());
        assert_eq!(error.code, ErrorCode::ResourceLimited);
        assert_eq!(error.storage.unwrap().available_bytes, 0);
        assert_eq!(first.metadata().unwrap().len(), 6);
        first.set_len(2).unwrap();
        second.write_all(b"yyyy").unwrap();
        assert_eq!(budget.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 10);
        drop(second);
        assert_eq!(budget.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 2);
        drop(first);
        assert_eq!(budget.usage().current_bytes, TEMPORARY_CONTROL_BYTES);
        assert_eq!(budget.usage().peak_bytes, TEMPORARY_CONTROL_BYTES + 10);
    }
    #[test]
    fn short_write_and_disk_full_refund_only_unused_reservation() {
        let dir = tempfile::tempdir().unwrap();
        let budget = TemporaryBudget::new(TEMPORARY_CONTROL_BYTES + 10, None).unwrap();
        let mut file = budget.temporary(dir.path()).unwrap();
        assert_eq!(
            file.write_with(b"abcdef", |f, b| f.write(&b[..2])).unwrap(),
            2
        );
        assert_eq!(budget.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 2);
        let error = file
            .write_with(b"abcd", |_, _| {
                Err(std::io::Error::from(std::io::ErrorKind::StorageFull))
            })
            .unwrap_err();
        assert_eq!(storage_io(error).code, ErrorCode::DiskFull);
        assert_eq!(budget.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 2);
        file.seek(SeekFrom::Start(9)).unwrap();
        assert_eq!(file.write_with(b"a", |_, _| Ok(0)).unwrap(), 0);
        file.seek(SeekFrom::Start(100)).unwrap();
        assert_eq!(file.write(&[]).unwrap(), 0);
        assert_eq!(file.metadata().unwrap().len(), 2);
        assert_eq!(budget.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 2);
    }
    #[test]
    fn dedup_refunds_duplicate_but_persisted_payload_stays_charged() {
        let dir = tempfile::tempdir().unwrap();
        let budget = TemporaryBudget::new(TEMPORARY_CONTROL_BYTES + 8, None).unwrap();
        let target = dir.path().join("retained");
        for expected in [true, false] {
            let mut file = budget.temporary(dir.path()).unwrap();
            file.write_all(b"same").unwrap();
            assert_eq!(file.persist(&target).unwrap(), expected);
            assert_eq!(budget.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 4);
        }
        assert_eq!(fs::read(target).unwrap(), b"same");
        assert_eq!(budget.usage().peak_bytes, TEMPORARY_CONTROL_BYTES + 8);
    }
    #[test]
    fn failed_unlink_keeps_capacity_reserved() {
        let dir = tempfile::tempdir().unwrap();
        let budget = TemporaryBudget::new(TEMPORARY_CONTROL_BYTES + 4, None).unwrap();
        let mut file = budget.temporary(dir.path()).unwrap();
        file.write_all(b"data").unwrap();
        let path = file.path().to_owned();
        fs::rename(&path, dir.path().join("residue")).unwrap();
        fs::create_dir(&path).unwrap();
        drop(file);
        assert_eq!(budget.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 4);
        assert_eq!(
            budget.reserve(1).unwrap_err().code,
            ErrorCode::ResourceLimited
        );
    }
}

#[cfg(test)]
mod external_tests {
    use super::*;
    #[test]
    fn external_path_replacement_cannot_refund_or_publish_another_inode() {
        let dir = tempfile::tempdir().unwrap();
        let disk = TemporaryBudget::new(TEMPORARY_CONTROL_BYTES + 32, None).unwrap();
        let lease = disk.external(&dir.path().join("output"), 24).unwrap();
        fs::write(lease.path(), b"partial").unwrap();
        fs::rename(lease.path(), dir.path().join("residue")).unwrap();
        fs::write(lease.path(), b"replacement").unwrap();
        assert_eq!(lease.finish().err().unwrap().code, ErrorCode::SourceChanged);
        assert_eq!(disk.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 24);
        assert_eq!(fs::read(dir.path().join("output")).unwrap(), b"replacement");
    }
    #[test]
    fn external_extent_is_reserved_before_writes_and_reconciled_after_reaping() {
        let dir = tempfile::tempdir().unwrap();
        let disk = TemporaryBudget::new(TEMPORARY_CONTROL_BYTES + 32, None).unwrap();
        let lease = disk.external(&dir.path().join("image.elf"), 24).unwrap();
        assert_eq!(disk.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 24);
        assert!(disk.external(&dir.path().join("other"), 9).is_err());
        fs::write(lease.path(), b"elf").unwrap(); // A reaped writer truncated its file.
        assert_eq!(disk.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 24);
        let file = lease.finish().unwrap();
        assert_eq!(disk.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 3);
        drop(file);
        assert_eq!(disk.usage().current_bytes, TEMPORARY_CONTROL_BYTES);
    }
    #[test]
    fn external_failure_and_cleanup_residue_preserve_admission() {
        let dir = tempfile::tempdir().unwrap();
        let disk = TemporaryBudget::new(TEMPORARY_CONTROL_BYTES + 32, None).unwrap();
        let lease = disk.external(&dir.path().join("output"), 24).unwrap();
        fs::write(lease.path(), b"partial").unwrap();
        drop(lease);
        assert_eq!(disk.usage().current_bytes, TEMPORARY_CONTROL_BYTES);
        let lease = disk.external(&dir.path().join("output"), 24).unwrap();
        fs::rename(lease.path(), dir.path().join("residue")).unwrap();
        drop(lease);
        assert_eq!(disk.usage().current_bytes, TEMPORARY_CONTROL_BYTES + 24);
    }
}
