//! Private content storage and atomic revision publication.
//!
//! A writer owns the import lock until dropped. Payloads become durable before
//! SQLite publishes their revision. Readers verify retained bytes, never origins.

mod preservation;
pub use preservation::*;
mod knowledge;
pub use knowledge::*;
mod investigations;
pub use investigations::*;
mod executions;
pub use executions::*;
mod functions;
pub use functions::{FunctionLease, RetainedFunction};
mod images;
pub use images::{ImageLease, RetainedImage};
mod temporary;
pub use temporary::{TemporaryBudget, TemporaryConfig, TemporaryControl, TemporaryFile};
mod records;
pub use records::{OwnerIdentity, PreparedImageReceipt, PreparedImport, RunOperation, RunRecord};
pub use records::{
    PreparedExecutionReceipt, PreparedFunctionReceipt, PreparedInvestigationReceipt,
    PreparedKnowledgeReceipt,
};
mod capture;
mod jobs;
pub use jobs::read_progress;
mod json_ranges;
mod metered;
mod query;
pub use query::{DoctorSink, DoctorSummary, ManifestLease, SnapshotView};
mod stream;
pub use stream::{InputStream, ObjectHeader, ObjectStream, RevisionStream};
mod source;
pub use jobs::{DoctorReport, RetainedImport, Staging};
pub use source::FileLease;
#[cfg(test)]
mod tests;
pub use capture::ArtifactLease;

use blobray_domain::*;
use rusqlite::{Connection, OpenFlags, params};
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};

const STATE: &str = ".blobray-next";
const SCHEMA: i64 = 9;

/// A project handle owns no source-file handles or mutable inventory cache.
#[derive(Clone)]
pub struct Project {
    root: PathBuf,
    id: ProjectId,
}

/// Exclusive import capability. Dropping it releases the OS lock, including on failure.
pub struct Writer {
    project: Project,
    _lock: File,
}

impl Drop for Writer {
    fn drop(&mut self) {
        // Closing our descriptor alone can leave flock held by a descriptor
        // inherited during a concurrent spawn. Only this capability owns the
        // writer lifetime; the child must not extend it until its exec/exit.
        let _ = self._lock.unlock();
    }
}

pub(crate) fn io(error: std::io::Error) -> Error {
    storage_io(error)
}
fn db(error: rusqlite::Error) -> Error {
    let code = match error.sqlite_error_code() {
        Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
            ErrorCode::Busy
        }
        Some(rusqlite::ErrorCode::DiskFull) => ErrorCode::DiskFull,
        _ => ErrorCode::Storage,
    };
    Error::new(code, error.to_string())
}
fn integrity(message: impl Into<String>) -> Error {
    Error::new(ErrorCode::Integrity, message)
}

pub(crate) fn sync_dir(path: &Path) -> Result<()> {
    #[cfg(unix)]
    File::open(path).and_then(|f| f.sync_all()).map_err(io)?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

impl Project {
    /// Initialize private schema-9 metadata with schema-1 revision manifests. An existing state directory is never reset.
    pub fn create(path: &Path) -> Result<Self> {
        fs::create_dir_all(path).map_err(io)?;
        let destination = path.join(STATE);
        if destination.symlink_metadata().is_ok() {
            return Err(Error::new(
                ErrorCode::AlreadyExists,
                "project state already exists",
            ));
        }
        let stage = tempfile::Builder::new()
            .prefix(".blobray-next-init-")
            .tempdir_in(path)
            .map_err(io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(stage.path(), fs::Permissions::from_mode(0o700)).map_err(io)?;
        }
        fs::create_dir(stage.path().join("objects")).map_err(io)?;
        fs::create_dir(stage.path().join("staging")).map_err(io)?;
        fs::write(stage.path().join(".gitignore"), b"*\n").map_err(io)?;
        File::open(stage.path().join(".gitignore"))
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        File::create(stage.path().join("writer.lock"))
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        let connection = Connection::open(stage.path().join("project.sqlite3")).map_err(db)?;
        connection.execute_batch("PRAGMA synchronous=EXTRA;
            BEGIN IMMEDIATE;
            PRAGMA user_version=9;
            CREATE TABLE legacy_imports (id TEXT PRIMARY KEY);
            CREATE TABLE knowledge_revisions (sequence INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, parent TEXT, assertion TEXT NOT NULL, action TEXT NOT NULL, supersedes TEXT);
            CREATE INDEX knowledge_assertion ON knowledge_revisions(assertion, sequence);
            CREATE INDEX knowledge_replacement ON knowledge_revisions(supersedes, sequence);
            CREATE TABLE publications (sequence INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, revision TEXT NOT NULL);
            CREATE TABLE current_publication (singleton INTEGER PRIMARY KEY CHECK(singleton=1), id TEXT NOT NULL);
            CREATE TABLE analyses (sequence INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, revision TEXT NOT NULL);
            CREATE TABLE images (sequence INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, revision TEXT NOT NULL, plan TEXT NOT NULL);
            CREATE TABLE runs (sequence INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE, record TEXT NOT NULL);
            CREATE TABLE project (singleton INTEGER PRIMARY KEY CHECK(singleton=1), id TEXT NOT NULL, current_revision TEXT);
            CREATE TABLE revisions (sequence INTEGER PRIMARY KEY, id TEXT NOT NULL UNIQUE);
            INSERT INTO project VALUES (1, lower(hex(randomblob(32))), NULL);
            COMMIT;").map_err(db)?;
        drop(connection);
        File::open(stage.path().join("project.sqlite3"))
            .and_then(|f| f.sync_all())
            .map_err(io)?;
        sync_dir(stage.path())?;
        fs::rename(stage.path(), &destination).map_err(io)?;
        sync_dir(path)?;
        Self::open(path)
    }

    /// Open existing storage without creating files or opening import origins.
    pub fn open(path: &Path) -> Result<Self> {
        let root = path.join(STATE);
        if !root.join("project.sqlite3").is_file() {
            return Err(Error::new(
                ErrorCode::NotFound,
                "project database does not exist; use init explicitly",
            ));
        }
        let connection = open_connection(&root, false)?;
        let raw: String = connection
            .query_row("SELECT id FROM project WHERE singleton=1", [], |r| r.get(0))
            .map_err(db)?;
        let id = raw
            .parse()
            .map_err(|_| integrity("invalid project identity"))?;
        Ok(Self { root, id })
    }

    pub fn id(&self) -> &ProjectId {
        &self.id
    }

    pub fn current(&self) -> Result<Option<RevisionId>> {
        let connection = open_connection(&self.root, false)?;
        let raw: Option<String> = connection
            .query_row(
                "SELECT current_revision FROM project WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(db)?;
        raw.map(|s| {
            s.parse()
                .map_err(|_| integrity("invalid current revision identity"))
        })
        .transpose()
    }

    /// Publication order; failed imports do not appear here.
    pub fn revisions(&self) -> Result<Vec<RevisionId>> {
        let connection = open_connection(&self.root, false)?;
        let mut statement = connection
            .prepare("SELECT id FROM revisions ORDER BY sequence")
            .map_err(db)?;
        statement
            .query_map([], |r| r.get::<_, String>(0))
            .map_err(db)?
            .map(|row| {
                row.map_err(db)?
                    .parse()
                    .map_err(|_| integrity("invalid revision identity"))
            })
            .collect()
    }

    /// Pin verified immutable bytes in memory. No storage garbage collection is implemented.
    pub fn lease(&self, artifact: &ArtifactId) -> Result<ArtifactLease> {
        let memory = WorkingMemory::new(DEFAULT_WORKING_BYTES)?;
        let mut control = || Ok(());
        let source = self.open_payload(artifact, &mut control)?;
        let bytes = read_scratch(&source, &memory, &mut control)?;
        let _copy = memory.reserve(bytes.len() as u64, RunPosition::default())?;
        let mut owned = Vec::new();
        owned.try_reserve_exact(bytes.len()).map_err(|_| {
            Error::new(
                ErrorCode::ResourceLimited,
                "host allocation refused payload",
            )
        })?;
        owned.extend_from_slice(&bytes);
        Ok(ArtifactLease::new(owned))
    }

    /// Read a committed manifest and verify every separately retained payload.
    pub fn snapshot(&self, requested: Option<&RevisionId>) -> Result<Snapshot> {
        let memory = WorkingMemory::new(DEFAULT_WORKING_BYTES)?;
        let mut control = || Ok(());
        let view = self.read_inventory(requested, &memory, &mut control, &mut ())?;
        let size = view.manifest().len();
        let _decoded = memory.reserve(
            size.checked_mul(64)
                .ok_or_else(|| Error::new(ErrorCode::ResourceLimited, "snapshot size overflow"))?,
            control.position(),
        )?;
        let bytes = read_scratch(view.manifest(), &memory, &mut control)?;
        let revision: Revision = serde_json::from_slice(&bytes).map_err(jobs::json)?;
        Ok(Snapshot {
            revision_id: view.revision_id,
            revision,
        })
    }

    pub fn writer(&self) -> Result<Writer> {
        let lock = writer_lock(&self.root)?;
        // Acquiring a writer is the explicit recovery boundary for a hot SQLite
        // rollback journal left by an interrupted metadata transaction.
        open_connection(&self.root, true)?;
        Ok(Writer {
            project: self.clone(),
            _lock: lock,
        })
    }

    fn object_path(&self, artifact: &ArtifactId) -> PathBuf {
        self.root.join("objects").join(artifact.as_str())
    }

    fn validate_revision(&self, revision: &Revision) -> Result<()> {
        revision.validate()?;
        if revision.project != self.id {
            return Err(integrity("manifest belongs to a different project"));
        }
        for capture in revision.captures() {
            if let Capture::Captured { artifact, length } = capture {
                let lease = self.lease(artifact)?;
                if lease.bytes().len() as u64 != *length {
                    return Err(integrity("captured length differs from retained payload"));
                }
            }
        }
        Ok(())
    }
}

impl Writer {
    /// Acquire the writer before opening SQLite for explicit import/recovery.
    /// Read-only project opening never repairs a hot rollback journal.
    pub fn open(path: &Path) -> Result<Self> {
        let root = path.join(STATE);
        if !root.join("project.sqlite3").is_file() {
            return Err(Error::new(
                ErrorCode::NotFound,
                "project database does not exist; use init explicitly",
            ));
        }
        let lock = writer_lock(&root)?;
        open_connection(&root, true)?;
        Ok(Self {
            project: Project::open(path)?,
            _lock: lock,
        })
    }

    /// Cloneable read handle; it owns no part of this writer's lock lifetime.
    pub fn project(&self) -> &Project {
        &self.project
    }

    /// Atomically make a durable manifest visible. A stale parent cannot overwrite current.
    pub fn commit(mut self, revision: Revision) -> Result<Snapshot> {
        self.project.validate_revision(&revision)?;
        let bytes = serde_json::to_vec(&revision).map_err(|e| integrity(e.to_string()))?;
        let id = RevisionId::of_manifest(&bytes);
        self.persist_bytes(&bytes)?;
        let mut connection = open_connection(&self.project.root, true)?;
        let transaction = connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(db)?;
        let current: Option<String> = transaction
            .query_row(
                "SELECT current_revision FROM project WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(db)?;
        if current.as_deref() != revision.parent.as_ref().map(RevisionId::as_str) {
            return Err(Error::new(ErrorCode::Busy, "revision parent is stale"));
        }
        transaction
            .execute("INSERT INTO revisions(id) VALUES (?1)", [id.as_str()])
            .map_err(db)?;
        transaction
            .execute(
                "UPDATE project SET current_revision=?1 WHERE singleton=1 AND id=?2",
                params![id.as_str(), self.project.id.as_str()],
            )
            .map_err(db)?;
        transaction.commit().map_err(db)?;
        Ok(Snapshot {
            revision_id: id,
            revision,
        })
    }
}

fn open_connection(root: &Path, writable: bool) -> Result<Connection> {
    let flags = if writable {
        OpenFlags::SQLITE_OPEN_READ_WRITE
    } else {
        OpenFlags::SQLITE_OPEN_READ_ONLY
    };
    let connection =
        Connection::open_with_flags(root.join("project.sqlite3"), flags).map_err(db)?;
    connection
        // A short-lived reader must not turn an ordinary writer transition into
        // a failed import. Writer admission itself remains nonblocking via flock.
        .busy_timeout(if writable {
            std::time::Duration::from_millis(250)
        } else {
            std::time::Duration::ZERO
        })
        .map_err(db)?;
    let schema: i64 = connection
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .map_err(db)?;
    if schema != SCHEMA {
        return Err(Error::new(
            ErrorCode::Incompatible,
            format!("unsupported project schema {schema}"),
        ));
    }
    if writable {
        connection
            .pragma_update(None, "synchronous", "EXTRA")
            .map_err(db)?;
    }
    Ok(connection)
}

fn writer_lock(root: &Path) -> Result<File> {
    let lock = OpenOptions::new()
        .read(true)
        .write(true)
        .open(root.join("writer.lock"))
        .map_err(io)?;
    lock.try_lock().map_err(|e| match e {
        std::fs::TryLockError::WouldBlock => Error::new(
            ErrorCode::Busy,
            "another import owns the project writer lock",
        ),
        std::fs::TryLockError::Error(error) => io(error),
    })?;
    Ok(lock)
}

mod usage;
