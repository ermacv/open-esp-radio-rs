//! Private portable snapshots. No origin access, binary interpretation or scheduler.
use super::*;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
const MAGIC: &[u8; 17] = b"BLOBRAY-BACKUP-1\n";
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct Entry {
    name: String,
    length: u64,
    digest: ArtifactId,
}
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PreservationSummary {
    pub project: ProjectId,
    pub objects: u64,
    pub bytes: u64,
}
fn copy(
    source: &mut impl Read,
    target: &mut impl Write,
    length: u64,
    control: &mut dyn RunControl,
) -> Result<ArtifactId> {
    let mut hash = Sha256::new();
    let mut remaining = length;
    let mut buffer = [0; WORK_BLOCK];
    while remaining > 0 {
        control.checkpoint(1)?;
        let n = remaining.min(buffer.len() as u64) as usize;
        source.read_exact(&mut buffer[..n]).map_err(io)?;
        control.bytes(n)?;
        target.write_all(&buffer[..n]).map_err(io)?;
        hash.update(&buffer[..n]);
        remaining -= n as u64;
    }
    format!("{:x}", hash.finalize()).parse()
}
fn append(
    path: &Path,
    name: String,
    target: &mut impl Write,
    control: &mut dyn RunControl,
) -> Result<u64> {
    let (digest, length) = super::metered::hash_file(path, control)?;
    let metadata = serde_json::to_vec(&Entry {
        name,
        length,
        digest: digest.clone(),
    })
    .map_err(jobs::json)?;
    target
        .write_all(&(metadata.len() as u32).to_le_bytes())
        .map_err(io)?;
    target.write_all(&metadata).map_err(io)?;
    if copy(&mut File::open(path).map_err(io)?, target, length, control)? != digest {
        return Err(integrity("snapshot payload changed during copying"));
    }
    Ok(length)
}
impl Project {
    /// Snapshot SQLite in a read transaction, then stream immutable CAS objects.
    /// Concurrent publication may add harmless extra objects; it cannot change the captured head.
    pub fn backup(
        &self,
        stage: &Path,
        disk: &TemporaryBudget,
        control: &mut dyn RunControl,
    ) -> Result<PreservationSummary> {
        let mut source = open_connection(&self.root, false)?;
        let transaction = source.transaction().map_err(db)?;
        let pages: u32 = transaction
            .pragma_query_value(None, "page_count", |r| r.get(0))
            .map_err(db)?;
        let page_size: u32 = transaction
            .pragma_query_value(None, "page_size", |r| r.get(0))
            .map_err(db)?;
        let capacity = u64::from(pages)
            .checked_mul(u64::from(page_size))
            .and_then(|n| n.checked_add(CONTROL_MESSAGE_BYTES as u64))
            .ok_or_else(|| integrity("snapshot size overflow"))?;
        disk.reserve(capacity)?;
        let snapshot = stage.join("snapshot.sqlite3");
        let mut destination = Connection::open(&snapshot).map_err(db)?;
        {
            let backup =
                rusqlite::backup::Backup::new(&transaction, &mut destination).map_err(db)?;
            loop {
                control.checkpoint(1)?;
                match backup.step(64).map_err(db)? {
                    rusqlite::backup::StepResult::Done => break,
                    rusqlite::backup::StepResult::More => (),
                    _ => return Err(Error::new(ErrorCode::Busy, "database snapshot busy")),
                }
            }
        }
        drop(destination);
        transaction.commit().map_err(db)?;
        let mut output = disk.create(&stage.join("backup.blobray"))?;
        output.write_all(MAGIC).map_err(io)?;
        let mut summary = PreservationSummary {
            project: self.id.clone(),
            objects: 0,
            bytes: append(&snapshot, "database".into(), &mut output, control)?,
        };
        for entry in fs::read_dir(self.root.join("objects")).map_err(io)? {
            control.checkpoint(1)?;
            let entry = entry.map_err(io)?;
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| integrity("invalid CAS filename"))?;
            let _: ArtifactId = name.parse()?;
            if !entry.file_type().map_err(io)?.is_file() {
                return Err(integrity("non-file in immutable CAS"));
            }
            summary.bytes = summary
                .bytes
                .checked_add(append(&entry.path(), name, &mut output, control)?)
                .ok_or_else(|| integrity("backup size overflow"))?;
            summary.objects += 1;
        }
        output.write_all(&0u32.to_le_bytes()).map_err(io)?;
        output.sync_all().map_err(io)?;
        fs::remove_file(snapshot).map_err(io)?;
        disk.release(capacity);
        Ok(summary)
    }
}
/// Extract only into a new private staging root. No path supplied by the bundle is followed.
/// The caller publishes the project only after this succeeds and doctor validates its closure.
pub fn restore_backup(
    bundle: &Path,
    destination: &Path,
    disk: &TemporaryBudget,
    memory: &WorkingMemory,
    control: &mut dyn RunControl,
) -> Result<PreservationSummary> {
    fs::create_dir(destination).map_err(io)?;
    let root = destination.join(STATE);
    fs::create_dir(&root).map_err(io)?;
    fs::create_dir(root.join("objects")).map_err(io)?;
    fs::create_dir(root.join("staging")).map_err(io)?;
    let mut source = File::open(bundle).map_err(io)?;
    let mut magic = [0; 17];
    source.read_exact(&mut magic).map_err(io)?;
    if &magic != MAGIC {
        return Err(integrity("unsupported backup header"));
    }
    let mut count = 0;
    let mut bytes = 0u64;
    let mut database = false;
    loop {
        control.checkpoint(1)?;
        let mut size = [0; 4];
        source.read_exact(&mut size).map_err(io)?;
        let size = u32::from_le_bytes(size) as usize;
        if size == 0 {
            break;
        }
        if size > 512 {
            return Err(integrity("backup entry header too large"));
        }
        let mut metadata = [0; 512];
        source.read_exact(&mut metadata[..size]).map_err(io)?;
        let entry: Entry = serde_json::from_slice(&metadata[..size]).map_err(jobs::json)?;
        let path = if entry.name == "database" {
            if database {
                return Err(integrity("repeated backup database"));
            }
            database = true;
            root.join("project.sqlite3")
        } else {
            let id: ArtifactId = entry.name.parse()?;
            if id != entry.digest {
                return Err(integrity("CAS name differs from entry digest"));
            }
            count += 1;
            root.join("objects").join(id.as_str())
        };
        let mut output = disk.create(&path)?;
        if copy(&mut source, &mut output, entry.length, control)? != entry.digest {
            return Err(integrity("backup entry digest mismatch"));
        }
        output.sync_all().map_err(io)?;
        bytes = bytes
            .checked_add(entry.length)
            .ok_or_else(|| integrity("restored size overflow"))?;
    }
    let mut tail = [0];
    if source.read(&mut tail).map_err(io)? != 0 || !database {
        return Err(integrity("backup has trailing data or lacks database"));
    }
    fs::write(root.join(".gitignore"), b"*\n").map_err(io)?;
    File::create(root.join("writer.lock")).map_err(io)?;
    let project = Project::open(destination)?;
    struct Verify;
    impl DoctorSink for Verify {
        fn error(&mut self, error: &Error, _: &mut dyn RunControl) -> Result<()> {
            Err(error.clone())
        }
        fn unfinished(&mut self, _: &RunId, _: &mut dyn RunControl) -> Result<()> {
            Ok(())
        }
    }
    project.doctor_stream(memory, control, &mut Verify)?;
    // A restored run cannot own a process/staging lease from the original machine.
    // Keep the byte-exact source journal as evidence before changing unfinished states.
    let connection = open_connection(&root, false)?;
    let schema: u32 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db)?;
    drop(connection);
    if schema >= 2 {
        let metadata_bytes = fs::metadata(root.join("project.sqlite3"))
            .map_err(io)?
            .len();
        disk.reserve(
            metadata_bytes
                .checked_mul(3)
                .ok_or_else(|| integrity("restore metadata budget overflow"))?,
        )?;
        let mut connection = Connection::open(root.join("project.sqlite3")).map_err(db)?;
        let tx = connection.transaction().map_err(db)?;
        let mut query = tx
            .prepare(
                "SELECT id,record FROM runs WHERE state IN ('registered','running','validating')",
            )
            .map_err(db)?;
        let mut rows = query.query([]).map_err(db)?;
        // Preserve the entire original database as a digest-qualified retained payload.
        let mut saved: Option<ArtifactId> = None;
        while let Some(row) = rows.next().map_err(db)? {
            control.checkpoint(1)?;
            if saved.is_none() {
                let original = root.join("project.sqlite3");
                let (id, length) = super::metered::hash_file(&original, control)?;
                let path = root.join("objects").join(id.as_str());
                if !path.exists() {
                    let mut output = disk.create(&path)?;
                    copy(
                        &mut File::open(&original).map_err(io)?,
                        &mut output,
                        length,
                        control,
                    )?;
                    output.sync_all().map_err(io)?;
                }
                saved = Some(id);
            }
            let raw: String = row.get(1).map_err(db)?;
            if raw.len() > CONTROL_MESSAGE_BYTES {
                return Err(integrity("restored run exceeds control limit"));
            }
            let mut run = jobs::decode_run(&raw)?;
            run.state = RunState::Abandoned;
            run.error = Some(Error::new(
                ErrorCode::RecoveryRequired,
                format!(
                    "unfinished operation belongs to the backup source; original journal payload {}",
                    saved.as_ref().unwrap()
                ),
            ));
            tx.execute(
                "UPDATE runs SET record=?2 WHERE id=?1",
                params![
                    run.id.as_str(),
                    serde_json::to_string(&run).map_err(jobs::json)?
                ],
            )
            .map_err(db)?;
        }
        drop(rows);
        drop(query);
        tx.commit().map_err(db)?;
    }
    sync_dir(&root.join("objects"))?;
    sync_dir(&root)?;
    sync_dir(destination)?;
    Ok(PreservationSummary {
        project: project.id,
        objects: count,
        bytes,
    })
}

impl Project {
    pub fn legacy_manifest(&self, control: &mut dyn RunControl) -> Result<Option<LegacyManifest>> {
        use rusqlite::OptionalExtension;
        let connection = open_connection(&self.root, false)?;
        let schema: u32 = connection
            .pragma_query_value(None, "user_version", |r| r.get(0))
            .map_err(db)?;
        if schema < 6 {
            return Ok(None);
        }
        let raw: Option<String> = connection
            .query_row("SELECT id FROM legacy_imports", [], |r| r.get(0))
            .optional()
            .map_err(db)?;
        let Some(raw) = raw else { return Ok(None) };
        let lease = self.open_payload(&raw.parse()?, control)?;
        if lease.len() > CONTROL_MESSAGE_BYTES as u64 {
            return Err(integrity("legacy manifest exceeds limit"));
        }
        let mut bytes = vec![0; lease.len() as usize];
        lease.read_at(0, &mut bytes, control)?;
        let manifest: LegacyManifest = serde_json::from_slice(&bytes).map_err(jobs::json)?;
        if manifest.schema != 1 || manifest.project != self.id {
            return Err(integrity("legacy manifest identity differs"));
        }
        Ok(Some(manifest))
    }
    pub fn visit_legacy(
        &self,
        control: &mut dyn RunControl,
        sink: &mut dyn FnMut(&LegacyRecord, &mut dyn RunControl) -> Result<()>,
    ) -> Result<Option<LegacyManifest>> {
        let manifest = self.legacy_manifest(control)?;
        if let Some(manifest) = &manifest {
            let mut last_capture = None;
            let mut count = 0;
            let mut converted = 0;
            let mut missing = 0;
            let mut unsupported = 0;
            visit_jsonl::<LegacyRecord>(
                &self.open_payload(&manifest.records, control)?,
                control,
                |record, c| {
                    count += 1;
                    if let Capture::Captured { artifact, length } = &record.capture
                        && last_capture.as_ref() != Some(&(artifact.clone(), *length))
                        && self.open_payload(artifact, c)?.len() != *length
                    {
                        return Err(integrity("legacy retained file length differs"));
                    }
                    if let Capture::Captured { artifact, length } = &record.capture {
                        last_capture = Some((artifact.clone(), *length));
                    }
                    match &record.outcome {
                        LegacyOutcome::Converted { revision } => {
                            self.knowledge_manifest(revision, c)?;
                            converted += 1;
                        }
                        LegacyOutcome::MissingPayload { .. } => missing += 1,
                        LegacyOutcome::Unsupported { .. } => unsupported += 1,
                        _ => (),
                    };
                    sink(&record, c)
                },
            )?;
            if (count, converted, missing, unsupported)
                != (
                    manifest.record_count,
                    manifest.converted,
                    manifest.missing,
                    manifest.unsupported,
                )
            {
                return Err(integrity("legacy catalog counts differ"));
            }
        }
        Ok(manifest)
    }
}
impl Writer {
    /// For a still-private new-project build. Callers expose it only after full validation.
    pub fn retain_legacy_catalog(
        &mut self,
        manifest: &LegacyManifest,
        stage: &Staging,
        control: &mut dyn RunControl,
    ) -> Result<()> {
        if manifest.project != self.project.id {
            return Err(integrity("legacy catalog belongs to another project"));
        }
        let mut last_capture = None;
        visit_jsonl::<LegacyRecord>(
            &stage.open_payload(&manifest.records, control)?,
            control,
            |record, c| {
                if let Capture::Captured { artifact, length } = record.capture
                    && last_capture.as_ref() != Some(&(artifact.clone(), length))
                {
                    self.promote(stage, &artifact, Some(length), c)?;
                    last_capture = Some((artifact, length));
                }
                Ok(())
            },
        )?;
        self.promote(stage, &manifest.records, None, control)?;
        let bytes = serde_json::to_vec(manifest).map_err(jobs::json)?;
        let mut file = stage.disk.temporary(&stage.root.join("staging"))?;
        file.write_all(&bytes).map_err(io)?;
        let id = stage.retain_temporary(file, control)?;
        self.promote(stage, &id, None, control)?;
        sync_dir(&self.project.root.join("objects"))?;
        open_connection(&self.project.root, true)?
            .execute("INSERT INTO legacy_imports(id) VALUES(?1)", [id.as_str()])
            .map_err(db)?;
        Ok(())
    }
}

impl Writer {
    /// Bound SQLite growth in a private new-project build before the next bounded event.
    /// The caller separately reserves rollback-journal and page-growth headroom.
    pub fn check_metadata_capacity(&self, maximum_bytes: u64) -> Result<()> {
        if fs::metadata(self.project.root.join("project.sqlite3"))
            .map_err(io)?
            .len()
            > maximum_bytes
        {
            return Err(Error::new(
                ErrorCode::ResourceLimited,
                "private project metadata capacity exhausted",
            ));
        }
        Ok(())
    }
}
