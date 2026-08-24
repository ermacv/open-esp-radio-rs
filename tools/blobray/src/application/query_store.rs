//! Persistent cache owned by the demand-driven analysis engine.
//!
//! SQLite owns query identity, dependency edges and result locations. Large
//! immutable values live in one append-only content-addressed pack so the
//! cache does not create one filesystem object per function or force SQLite's
//! WAL to carry large analysis payloads.

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    ops::{Deref, DerefMut},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::{Duration, SystemTime},
};

use rusqlite::{Connection, OpenFlags, OptionalExtension, config::DbConfig, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::Result;

type PackedObjectLocations = Vec<(String, u64, u64)>;

const STORE_SCHEMA: i64 = 7;
const INLINE_VALUE_LIMIT: usize = 64 * 1024;
// Stay comfortably below SQLite's host-parameter limit while amortizing one
// lookup across many function keys. This also bounds the dynamically prepared
// statement and the number of result rows held at once.
const FUNCTION_FACT_LOOKUP_BATCH: usize = 256;
const PACK_RECORD_MAGIC: &[u8; 8] = b"BLBRCAS1";
const PACK_HEADER_BYTES: u64 = 8 + 32 + 8;
const COMPACT_MIN_PACK_BYTES: u64 = 256 * 1024 * 1024;
const COMPACT_MIN_RECLAIMABLE_BYTES: u64 = 64 * 1024 * 1024;
const COMPACT_MIN_RECLAIMABLE_PERCENT: u8 = 25;
static NEXT_RESTORE_ID: AtomicU64 = AtomicU64::new(0);

struct PreparedFunctionFact<'a> {
    query_key: &'a str,
    value: &'a [u8],
    result_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct QueryKindStatistics {
    pub(crate) kind: String,
    pub(crate) query_results: u64,
    pub(crate) inline_bytes: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct QueryStoreStatistics {
    pub(crate) present: bool,
    pub(crate) cache_root: PathBuf,
    pub(crate) database_path: PathBuf,
    pub(crate) schema: Option<u32>,
    pub(crate) root_bytes: u64,
    pub(crate) database_bytes: u64,
    pub(crate) pack_bytes: u64,
    pub(crate) query_results: u64,
    pub(crate) query_kinds: Vec<QueryKindStatistics>,
    pub(crate) inline_bytes: u64,
    pub(crate) dependencies: u64,
    pub(crate) objects: u64,
    pub(crate) object_payload_bytes: u64,
    pub(crate) stage_bindings: u64,
    pub(crate) stage_outputs: u64,
    pub(crate) live_objects: u64,
    pub(crate) live_record_bytes: u64,
    pub(crate) reclaimable_pack_bytes: u64,
    pub(crate) compaction: QueryStoreCompactionStatistics,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub(crate) struct QueryStoreCompactionStatistics {
    pub(crate) supported: bool,
    pub(crate) automatic: bool,
    pub(crate) eligible_on_next_write: bool,
    pub(crate) minimum_pack_bytes: u64,
    pub(crate) minimum_reclaimable_bytes: u64,
    pub(crate) minimum_reclaimable_percent: u8,
}

impl QueryStoreStatistics {
    pub(crate) fn empty(cache_root: PathBuf, database_path: PathBuf) -> Self {
        Self {
            present: false,
            cache_root,
            database_path,
            schema: None,
            root_bytes: 0,
            database_bytes: 0,
            pack_bytes: 0,
            query_results: 0,
            query_kinds: Vec::new(),
            inline_bytes: 0,
            dependencies: 0,
            objects: 0,
            object_payload_bytes: 0,
            stage_bindings: 0,
            stage_outputs: 0,
            live_objects: 0,
            live_record_bytes: 0,
            reclaimable_pack_bytes: 0,
            compaction: compaction_statistics(0, 0),
        }
    }
}

fn compaction_statistics(
    pack_bytes: u64,
    reclaimable_pack_bytes: u64,
) -> QueryStoreCompactionStatistics {
    let supported = cfg!(target_os = "linux");
    let eligible_on_next_write = supported
        && pack_bytes >= COMPACT_MIN_PACK_BYTES
        && reclaimable_pack_bytes >= COMPACT_MIN_RECLAIMABLE_BYTES
        && reclaimable_pack_bytes.saturating_mul(100)
            >= pack_bytes.saturating_mul(u64::from(COMPACT_MIN_RECLAIMABLE_PERCENT));
    QueryStoreCompactionStatistics {
        supported,
        automatic: supported,
        eligible_on_next_write,
        minimum_pack_bytes: COMPACT_MIN_PACK_BYTES,
        minimum_reclaimable_bytes: COMPACT_MIN_RECLAIMABLE_BYTES,
        minimum_reclaimable_percent: COMPACT_MIN_RECLAIMABLE_PERCENT,
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CacheTreeFingerprint {
    entries: Vec<CacheTreeEntry>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CacheTreeEntryKind {
    Directory,
    File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CacheTreeEntry {
    path: PathBuf,
    kind: CacheTreeEntryKind,
    len: u64,
    modified: Option<SystemTime>,
    readonly: bool,
}

impl CacheTreeFingerprint {
    fn file_len(&self, path: &Path) -> Option<u64> {
        self.entries
            .iter()
            .find(|entry| entry.kind == CacheTreeEntryKind::File && entry.path == path)
            .map(|entry| entry.len)
    }

    fn file_bytes(&self) -> Result<u64> {
        self.entries
            .iter()
            .filter(|entry| entry.kind == CacheTreeEntryKind::File)
            .try_fold(0_u64, |total, entry| {
                total.checked_add(entry.len).ok_or_else(|| {
                    crate::Error::invalid("query-cache root byte count overflowed u64")
                })
            })
    }

    fn pack_bytes(&self) -> Result<u64> {
        self.entries
            .iter()
            .filter(|entry| {
                entry.kind == CacheTreeEntryKind::File
                    && entry.path.components().count() == 1
                    && entry
                        .path
                        .file_name()
                        .and_then(|name| name.to_str())
                        .is_some_and(is_pack_name)
            })
            .try_fold(0_u64, |total, entry| {
                total.checked_add(entry.len).ok_or_else(|| {
                    crate::Error::invalid("query-cache pack byte count overflowed u64")
                })
            })
    }

    fn has_nonempty_wal(&self) -> bool {
        self.entries.iter().any(|entry| {
            entry.kind == CacheTreeEntryKind::File
                && entry.path == Path::new("queries.sqlite3-wal")
                && entry.len != 0
        })
    }
}

pub(crate) struct QueryStore {
    connection: PinnedConnection,
    /// Stable user-facing path. Blobray's direct filesystem and pack access
    /// goes through `storage_root`; on Linux that root is held by a directory
    /// descriptor. SQLite is opened there but may canonicalize it internally.
    root: PathBuf,
    root_identity: CacheRootIdentity,
    storage_root: PathBuf,
    pack_path: PathBuf,
    next_pack_generation: u64,
    _root_pin: PinnedCacheRoot,
    _access_lock: File,
}

/// Lifetime guard for the persistent-cache snapshot seen by a plan.
///
/// The database lock excludes a writer for that generation. On Linux the
/// directory handle pins direct file and pack reads to the same directory even
/// if the user-facing cache path is renamed and replaced. SQLite may resolve
/// sidecar names through a canonical pathname; permanent replacement is
/// rejected by binding checks, but closing that narrow race completely would
/// require a custom VFS. Other targets retain identity checks as a best-effort
/// guard and disable destructive pack compaction/cleanup until they have an
/// equivalent handle-relative backend.
#[derive(Debug)]
pub(crate) struct PlanReadGuard {
    root: PathBuf,
    root_identity: CacheRootIdentity,
    pinned_root: PinnedCacheRoot,
    database_path: PathBuf,
    access_lock: File,
}

#[derive(Debug)]
struct PinnedCacheRoot {
    storage_root: PathBuf,
    #[cfg(target_os = "linux")]
    directory: File,
}

/// Writer-close hardening for a database opened through the pinned root.
///
/// Binding validation and `NO_CKPT_ON_CLOSE` keep permanent root replacement
/// fail-closed. This does not replace SQLite's pathname-based sidecar VFS, so
/// it is not a complete defense against a hostile replacement-and-restore ABA.
struct PinnedConnection {
    connection: Option<Connection>,
    cleanup: Option<PinnedConnectionCleanup>,
}

struct PinnedConnectionCleanup {
    root: PathBuf,
    root_identity: CacheRootIdentity,
    storage_database_path: PathBuf,
    database_file: File,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct CacheRootIdentity {
    parent: DirectoryIdentity,
    root: DirectoryIdentity,
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DirectoryIdentity {
    canonical: PathBuf,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(not(unix))]
    created: Option<SystemTime>,
}

impl QueryStore {
    /// Pin the cache-root generation observed by a complete plan and hold its
    /// process-wide shared database lock without creating state. The pin is
    /// descriptor-backed on Linux and identity-checked elsewhere.
    pub(crate) fn plan_read_guard(project_manifest: &Path) -> Result<Option<PlanReadGuard>> {
        let project_root = project_manifest.parent().unwrap_or_else(|| Path::new("."));
        let cache_root = project_root.join("generated/.blobray-cache");
        let database_path = cache_root.join("queries.sqlite3");
        let root_metadata = match fs::symlink_metadata(&cache_root) {
            Ok(metadata) => metadata,
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                validate_absent_cache_root_parent(&cache_root)?;
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(crate::Error::invalid(format!(
                "query cache root {} is not a regular directory",
                cache_root.display()
            )));
        }
        let root_identity = CacheRootIdentity::capture(&cache_root)?;
        let pinned_root = PinnedCacheRoot::open(&cache_root, &root_identity)?;
        let storage_database_path = pinned_root.storage_root.join("queries.sqlite3");
        let database_metadata = match fs::symlink_metadata(&storage_database_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                validate_cold_cache_root(&pinned_root.storage_root)?;
                root_identity.validate(&cache_root)?;
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        if database_metadata.file_type().is_symlink() || !database_metadata.is_file() {
            return Err(crate::Error::invalid(format!(
                "query cache database {} is not a regular file",
                database_path.display()
            )));
        }
        let access_lock = open_cache_database_read_only(&storage_database_path)?;
        access_lock
            .try_lock_shared()
            .map_err(|error| cache_lock_error("read", &database_path, error))?;
        root_identity.validate(&cache_root)?;
        Ok(Some(PlanReadGuard {
            root: cache_root,
            root_identity,
            pinned_root,
            database_path,
            access_lock,
        }))
    }

    /// Read one stage result from the generation pinned by `guard` without
    /// creating, migrating, repairing, restoring, or rebinding state.
    ///
    /// Blobray resolves the database and pack through the pinned root. SQLite
    /// may canonicalize the database pathname internally; root and database
    /// binding checks reject a persistent lexical-root replacement before or
    /// after the read.
    pub(crate) fn stage_output_digests_read_only(
        guard: &PlanReadGuard,
        query_key: &str,
        validate_payloads: bool,
    ) -> Result<Option<Vec<String>>> {
        guard.validate_lexical_root()?;
        let storage_database_path = guard.pinned_root.storage_root.join("queries.sqlite3");
        let preflight = fingerprint_cache_tree(&guard.pinned_root.storage_root)?;
        reject_nonempty_wal(&preflight, &guard.database_path)?;
        let connection = Connection::open_with_flags(
            immutable_database_uri_pinned(&storage_database_path)?,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|error| store_error("open query database read-only", error))?;
        let schema = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .map_err(|error| store_error("read query database schema", error))?;
        if schema != STORE_SCHEMA {
            // Write-mode opening deliberately replaces an obsolete schema.
            // A plan must remain read-only, but it should predict the same
            // cold computation instead of treating replaceable cache state as
            // a project blocker.
            drop(connection);
            let postflight = fingerprint_cache_tree(&guard.pinned_root.storage_root)?;
            reject_nonempty_wal(&postflight, &guard.database_path)?;
            if postflight != preflight {
                return Err(crate::Error::invalid(
                    "query cache changed during read-only stage inspection; retry after the cache writer exits",
                ));
            }
            guard.validate_lexical_root()?;
            return Ok(None);
        }
        let (active_pack, next_pack_generation) = connection
            .query_row(
                "SELECT active_pack, next_pack_generation FROM cache_state WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|error| store_error("read query-cache state", error))?;
        if !is_pack_name(&active_pack) || next_pack_generation < 0 {
            return Err(crate::Error::invalid(
                "query cache has an invalid active pack or generation",
            ));
        }
        let root_pin = guard.pinned_root.try_clone()?;
        let storage_root = root_pin.storage_root.clone();
        let store = Self {
            connection: PinnedConnection::read_only(connection),
            root: guard.root.clone(),
            root_identity: guard.root_identity.clone(),
            storage_root,
            pack_path: guard.root.join(active_pack),
            next_pack_generation: next_pack_generation as u64,
            _root_pin: root_pin,
            _access_lock: guard.access_lock.try_clone()?,
        };
        let result = store.stage_output_digests(query_key)?;
        if validate_payloads && let Some(digests) = result.as_ref() {
            // A cache record is restorable only when every generated output
            // still has a complete payload matching its content digest.
            for digest in digests {
                store.validate_object_payload(digest)?;
            }
        }
        drop(store);

        let postflight = fingerprint_cache_tree(&guard.pinned_root.storage_root)?;
        reject_nonempty_wal(&postflight, &guard.database_path)?;
        if postflight != preflight {
            return Err(crate::Error::invalid(
                "query cache changed during read-only stage inspection; retry after the cache writer exits",
            ));
        }
        guard.validate_lexical_root()?;
        Ok(result)
    }

    /// Inspect persistent cache storage without creating, migrating or
    /// repairing it.
    ///
    /// A missing cache is a valid empty state. Once a database exists its
    /// schema and every statistics query must be readable at the current
    /// version; incompatible or corrupt state fails closed. The connection is
    /// explicitly read-only so this path cannot create the database or reset
    /// an older schema as [`Self::open`] intentionally does for write mode.
    pub(crate) fn statistics(project_manifest: &Path) -> Result<QueryStoreStatistics> {
        Self::statistics_after_preflight(project_manifest, || ())
    }

    fn statistics_after_preflight<T>(
        project_manifest: &Path,
        after_preflight: impl FnOnce() -> T,
    ) -> Result<QueryStoreStatistics> {
        let project_root = project_manifest.parent().unwrap_or_else(|| Path::new("."));
        let cache_root = project_root.join("generated/.blobray-cache");
        let database_path = cache_root.join("queries.sqlite3");
        let empty = || QueryStoreStatistics::empty(cache_root.clone(), database_path.clone());

        let root_metadata = match fs::symlink_metadata(&cache_root) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(empty()),
            Err(error) => return Err(error.into()),
        };
        if root_metadata.file_type().is_symlink() || !root_metadata.is_dir() {
            return Err(crate::Error::invalid(format!(
                "query cache root {} is not a regular directory",
                cache_root.display()
            )));
        }
        let root_identity = CacheRootIdentity::capture(&cache_root)?;
        let root_pin = PinnedCacheRoot::open(&cache_root, &root_identity)?;
        let storage_root = &root_pin.storage_root;
        let storage_database_path = storage_root.join("queries.sqlite3");

        let database_metadata = match fs::symlink_metadata(&storage_database_path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                if fs::read_dir(storage_root)?.next().is_none() {
                    return Ok(empty());
                }
                return Err(crate::Error::invalid(format!(
                    "query cache root {} exists without queries.sqlite3",
                    cache_root.display()
                )));
            }
            Err(error) => return Err(error.into()),
        };
        if database_metadata.file_type().is_symlink() || !database_metadata.is_file() {
            return Err(crate::Error::invalid(format!(
                "query cache database {} is not a regular file",
                database_path.display()
            )));
        }

        // Measure before opening SQLite. Besides making the reported physical
        // state one coherent snapshot, this ensures an accidental sidecar
        // creation cannot be hidden from read-only regression tests.
        let preflight = fingerprint_cache_tree(storage_root)?;
        reject_nonempty_wal(&preflight, &database_path)?;
        let root_bytes = preflight.file_bytes()?;
        let database_bytes = preflight
            .file_len(Path::new("queries.sqlite3"))
            .ok_or_else(|| crate::Error::invalid("query cache database disappeared"))?;
        let pack_bytes = preflight.pack_bytes()?;
        let _postflight_guard = after_preflight();

        let database_uri = immutable_database_uri_pinned(&storage_database_path)?;
        let connection = Connection::open_with_flags(
            database_uri,
            OpenFlags::SQLITE_OPEN_READ_ONLY
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_URI,
        )
        .map_err(|error| store_error("open query database read-only", error))?;
        let schema = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .map_err(|error| store_error("read query database schema", error))?;
        if schema != STORE_SCHEMA {
            return Err(crate::Error::invalid(format!(
                "query cache schema {schema} is unsupported; expected {STORE_SCHEMA}"
            )));
        }
        let schema = u32::try_from(schema)
            .map_err(|_| crate::Error::invalid("query cache schema is outside u32"))?;

        let state_rows = query_nonnegative_count(
            &connection,
            "count query-cache state rows",
            "SELECT COUNT(*) FROM cache_state",
        )?;
        if state_rows != 1 {
            return Err(crate::Error::invalid(format!(
                "query cache has {state_rows} cache-state rows; expected exactly one"
            )));
        }
        let (active_pack, next_pack_generation) = connection
            .query_row(
                "SELECT active_pack, next_pack_generation FROM cache_state WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|error| store_error("read query-cache state", error))?;
        if !is_pack_name(&active_pack) || next_pack_generation < 0 {
            return Err(crate::Error::invalid(
                "query cache has an invalid active pack or generation",
            ));
        }

        let (query_results, inline_bytes) = query_nonnegative_pair(
            &connection,
            "measure query results",
            "SELECT COUNT(*), COALESCE(SUM(LENGTH(inline_value)), 0) FROM query_results",
        )?;
        let mut query_kinds = Vec::new();
        {
            let mut statement = connection
                .prepare(
                    "SELECT kind, COUNT(*), COALESCE(SUM(LENGTH(inline_value)), 0)
                     FROM query_results GROUP BY kind ORDER BY kind",
                )
                .map_err(|error| store_error("prepare query-kind statistics", error))?;
            let mut rows = statement
                .query([])
                .map_err(|error| store_error("read query-kind statistics", error))?;
            while let Some(row) = rows
                .next()
                .map_err(|error| store_error("advance query-kind statistics", error))?
            {
                let kind = row
                    .get::<_, String>(0)
                    .map_err(|error| store_error("decode query kind", error))?;
                let count = row
                    .get::<_, i64>(1)
                    .map_err(|error| store_error("decode query-kind count", error))?;
                let bytes = row
                    .get::<_, i64>(2)
                    .map_err(|error| store_error("decode query-kind inline bytes", error))?;
                query_kinds.push(QueryKindStatistics {
                    kind,
                    query_results: nonnegative(count, "query-kind result count")?,
                    inline_bytes: nonnegative(bytes, "query-kind inline bytes")?,
                });
            }
        }

        let dependencies = query_nonnegative_count(
            &connection,
            "count query dependencies",
            "SELECT COUNT(*) FROM query_dependencies",
        )?;
        let (objects, object_payload_bytes) = query_nonnegative_pair(
            &connection,
            "measure query-cache objects",
            "SELECT COUNT(*), COALESCE(SUM(payload_length), 0) FROM objects",
        )?;
        let invalid_objects = query_nonnegative_count(
            &connection,
            "validate query-cache object locations",
            "SELECT COUNT(*) FROM objects WHERE pack_offset < 0 OR payload_length < 0",
        )?;
        if invalid_objects != 0 {
            return Err(crate::Error::invalid(format!(
                "query cache has {invalid_objects} object(s) with invalid locations"
            )));
        }
        validate_indexed_pack_extents(&connection, storage_root)?;

        let stage_bindings = query_nonnegative_count(
            &connection,
            "count cached stage bindings",
            "SELECT COUNT(*) FROM stage_bindings",
        )?;
        let stage_outputs = query_nonnegative_count(
            &connection,
            "count cached stage outputs",
            "SELECT COUNT(*) FROM stage_outputs",
        )?;
        let (live_references, live_objects, live_record_bytes) = connection
            .query_row(
                "WITH live(digest) AS (
                     SELECT object_digest FROM query_results WHERE object_digest IS NOT NULL
                     UNION
                     SELECT digest FROM stage_outputs
                 )
                 SELECT COUNT(*), COUNT(objects.digest),
                        COALESCE(SUM(?1 + objects.payload_length), 0)
                 FROM live LEFT JOIN objects USING (digest)",
                [PACK_HEADER_BYTES as i64],
                |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .map_err(|error| store_error("measure live query-cache objects", error))?;
        let live_references = nonnegative(live_references, "live object reference count")?;
        let live_objects = nonnegative(live_objects, "live object count")?;
        let live_record_bytes = nonnegative(live_record_bytes, "live object record bytes")?;
        if live_references != live_objects {
            return Err(crate::Error::invalid(format!(
                "query cache references {} missing live object(s)",
                live_references - live_objects
            )));
        }
        let reclaimable_pack_bytes = pack_bytes.checked_sub(live_record_bytes).ok_or_else(|| {
            crate::Error::invalid(format!(
                "query cache reports {live_record_bytes} live record bytes in {pack_bytes} pack bytes"
            ))
        })?;

        let postflight = fingerprint_cache_tree(storage_root)?;
        reject_nonempty_wal(&postflight, &database_path)?;
        if postflight != preflight {
            return Err(crate::Error::invalid(
                "query cache changed during read-only statistics inspection; retry after the cache writer exits",
            ));
        }
        root_identity.validate(&cache_root)?;
        Ok(QueryStoreStatistics {
            present: true,
            cache_root,
            database_path,
            schema: Some(schema),
            root_bytes,
            database_bytes,
            pack_bytes,
            query_results,
            query_kinds,
            inline_bytes,
            dependencies,
            objects,
            object_payload_bytes,
            stage_bindings,
            stage_outputs,
            live_objects,
            live_record_bytes,
            reclaimable_pack_bytes,
            compaction: compaction_statistics(pack_bytes, reclaimable_pack_bytes),
        })
    }

    pub(crate) fn open(project_manifest: &Path) -> Result<Self> {
        let project_root = project_manifest.parent().unwrap_or_else(|| Path::new("."));
        let root = project_root.join("generated/.blobray-cache");
        ensure_cache_root(&root)?;
        let root_identity = CacheRootIdentity::capture(&root)?;
        let root_pin = PinnedCacheRoot::open(&root, &root_identity)?;
        let storage_root = root_pin.storage_root.clone();
        let database_path = root.join("queries.sqlite3");
        let storage_database_path = storage_root.join("queries.sqlite3");
        validate_sqlite_sidecars(&storage_database_path)?;
        let access_lock = open_cache_database(&storage_database_path)?;
        access_lock
            .try_lock()
            .map_err(|error| cache_lock_error("write", &database_path, error))?;
        let connection = Connection::open(&storage_database_path)
            .map_err(|error| store_error("open query database", error))?;
        verify_open_file_path(&access_lock, &storage_database_path, "query cache database")?;
        root_identity.validate(&root)?;
        validate_sqlite_sidecars(&storage_database_path)?;
        let connection = PinnedConnection::writer(
            connection,
            root.clone(),
            root_identity.clone(),
            storage_database_path,
            access_lock.try_clone()?,
        )?;
        connection
            .busy_timeout(Duration::from_secs(30))
            .map_err(|error| store_error("configure query database timeout", error))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
                 PRAGMA synchronous=NORMAL;
                 PRAGMA foreign_keys=ON;",
            )
            .map_err(|error| store_error("configure query database", error))?;

        let schema = connection
            .query_row("PRAGMA user_version", [], |row| row.get::<_, i64>(0))
            .map_err(|error| store_error("read query database schema", error))?;
        if schema != 0 && schema != STORE_SCHEMA {
            connection
                .execute_batch(
                    "DROP TABLE IF EXISTS stage_outputs;
                     DROP TABLE IF EXISTS stage_bindings;
                     DROP TABLE IF EXISTS query_dependencies;
                     DROP TABLE IF EXISTS query_results;
                     DROP TABLE IF EXISTS objects;
                     DROP TABLE IF EXISTS cache_state;",
                )
                .map_err(|error| store_error("reset obsolete query database", error))?;
            #[cfg(target_os = "linux")]
            remove_pack_files(&storage_root)?;
        }
        connection
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS objects (
                     digest TEXT PRIMARY KEY,
                     pack_name TEXT NOT NULL,
                     pack_offset INTEGER NOT NULL,
                     payload_length INTEGER NOT NULL
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS query_results (
                     query_key TEXT PRIMARY KEY,
                     kind TEXT NOT NULL,
                     input_fingerprint TEXT NOT NULL,
                     result_digest TEXT NOT NULL,
                     inline_value BLOB,
                     object_digest TEXT REFERENCES objects(digest),
                     CHECK ((inline_value IS NULL) != (object_digest IS NULL))
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS query_dependencies (
                     query_key TEXT NOT NULL REFERENCES query_results(query_key) ON DELETE CASCADE,
                     dependency_key TEXT NOT NULL,
                     PRIMARY KEY (query_key, dependency_key)
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS stage_bindings (
                     stage TEXT PRIMARY KEY,
                     query_key TEXT NOT NULL
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS stage_outputs (
                     stage TEXT NOT NULL,
                     path TEXT NOT NULL,
                     digest TEXT NOT NULL,
                     PRIMARY KEY (stage, path)
                 ) WITHOUT ROWID;
                 CREATE TABLE IF NOT EXISTS cache_state (
                     singleton INTEGER PRIMARY KEY CHECK (singleton = 1),
                     active_pack TEXT NOT NULL,
                     next_pack_generation INTEGER NOT NULL
                 );
                 INSERT OR IGNORE INTO cache_state(singleton, active_pack, next_pack_generation)
                 VALUES (1, 'objects-0.pack', 1);
                 PRAGMA user_version=7;",
            )
            .map_err(|error| store_error("initialize query database", error))?;
        let (active_pack, next_pack_generation) = connection
            .query_row(
                "SELECT active_pack, next_pack_generation FROM cache_state WHERE singleton = 1",
                [],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|error| store_error("read active query-cache pack", error))?;
        let next_pack_generation = u64::try_from(next_pack_generation).map_err(|_| {
            crate::Error::invalid("query cache has a negative next pack generation")
        })?;
        if !is_pack_name(&active_pack) {
            return Err(crate::Error::invalid(format!(
                "query cache has invalid active pack name {active_pack:?}"
            )));
        }
        let mut store = Self {
            connection,
            pack_path: root.join(active_pack),
            root,
            root_identity,
            storage_root,
            next_pack_generation,
            _root_pin: root_pin,
            _access_lock: access_lock,
        };
        store.remove_unreferenced_pack_files()?;
        Ok(store)
    }

    fn validate_root_identity(&self) -> Result<()> {
        self.root_identity.validate(&self.root)?;
        verify_open_file_path(
            &self._access_lock,
            &self.storage_root.join("queries.sqlite3"),
            "query cache database",
        )?;
        self.root_identity.validate(&self.root)
    }

    fn active_storage_pack_path(&self) -> Result<PathBuf> {
        let name = self
            .pack_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| crate::Error::invalid("query cache pack has no UTF-8 file name"))?;
        if self.pack_path.parent() != Some(self.root.as_path()) || !is_pack_name(name) {
            return Err(crate::Error::invalid(format!(
                "query cache has invalid active pack path {}",
                self.pack_path.display()
            )));
        }
        Ok(self.storage_root.join(name))
    }

    pub(crate) fn stage_output_digests(&self, query_key: &str) -> Result<Option<Vec<String>>> {
        self.validate_root_identity()?;
        let kind = self
            .connection
            .query_row(
                "SELECT kind FROM query_results WHERE query_key = ?1",
                [query_key],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| store_error("read cached stage result kind", error))?;
        let Some(kind) = kind else {
            return Ok(None);
        };
        if kind != "project-stage" {
            return Err(crate::Error::invalid(format!(
                "query {query_key:?} is {kind:?}, not a project-stage result"
            )));
        }
        self.get(query_key)?
            .map(|value| serde_json::from_slice(&value).map_err(Into::into))
            .transpose()
    }

    pub(crate) fn record_stage(
        &mut self,
        stage: &str,
        query_key: &str,
        outputs: &[(String, String, PathBuf)],
    ) -> Result<()> {
        self.validate_root_identity()?;
        let content_digests = outputs
            .iter()
            .map(|(_, digest, _)| digest.clone())
            .collect::<Vec<_>>();
        self.ensure_file_objects(outputs)?;
        let value = serde_json::to_vec(&content_digests)?;
        self.put(query_key, "project-stage", query_key, &[], &value)?;
        let paths = outputs
            .iter()
            .map(|(path, _, _)| path.clone())
            .collect::<Vec<_>>();
        self.bind_stage(stage, query_key, &paths, &content_digests)?;
        self.compact_if_needed()
    }

    pub(crate) fn bind_restored_stage(
        &mut self,
        stage: &str,
        query_key: &str,
        paths: &[String],
        content_digests: &[String],
    ) -> Result<()> {
        self.validate_root_identity()?;
        if paths.len() != content_digests.len() {
            return Err(crate::Error::invalid(format!(
                "cached stage {stage:?} has {} paths for {} output digests",
                paths.len(),
                content_digests.len()
            )));
        }
        self.bind_stage(stage, query_key, paths, content_digests)
    }

    /// Remove a stage publication only if it still names `query_key`.
    ///
    /// Input mutation checks use this after a post-commit validation fails.
    /// The immutable query result is retired only when no other stage binding
    /// owns it; pack payloads remain harmless orphans until later compaction.
    pub(crate) fn retire_stage_binding(&mut self, stage: &str, query_key: &str) -> Result<()> {
        self.validate_root_identity()?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| store_error("begin stage-binding retirement transaction", error))?;
        let retired = transaction
            .execute(
                "DELETE FROM stage_bindings WHERE stage = ?1 AND query_key = ?2",
                params![stage, query_key],
            )
            .map_err(|error| store_error("retire cached stage binding", error))?;
        if retired != 0 {
            transaction
                .execute("DELETE FROM stage_outputs WHERE stage = ?1", [stage])
                .map_err(|error| store_error("retire cached stage outputs", error))?;
            let remaining_bindings = transaction
                .query_row(
                    "SELECT COUNT(*) FROM stage_bindings WHERE query_key = ?1",
                    [query_key],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| store_error("count remaining cached stage owners", error))?;
            if remaining_bindings == 0 {
                transaction
                    .execute(
                        "DELETE FROM query_results WHERE query_key = ?1",
                        [query_key],
                    )
                    .map_err(|error| store_error("retire unowned cached stage result", error))?;
            }
        }
        transaction
            .commit()
            .map_err(|error| store_error("commit stage-binding retirement", error))?;
        self.validate_root_identity()
    }

    fn bind_stage(
        &mut self,
        stage: &str,
        query_key: &str,
        paths: &[String],
        content_digests: &[String],
    ) -> Result<()> {
        self.validate_root_identity()?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| store_error("begin cached stage transaction", error))?;
        let previous = transaction
            .query_row(
                "SELECT query_key FROM stage_bindings WHERE stage = ?1",
                [stage],
                |row| row.get::<_, String>(0),
            )
            .optional()
            .map_err(|error| store_error("read previous cached stage binding", error))?;
        transaction
            .execute(
                "INSERT INTO stage_bindings(stage, query_key) VALUES (?1, ?2)
                 ON CONFLICT(stage) DO UPDATE SET query_key = excluded.query_key",
                params![stage, query_key],
            )
            .map_err(|error| store_error("record cached stage binding", error))?;
        transaction
            .execute("DELETE FROM stage_outputs WHERE stage = ?1", [stage])
            .map_err(|error| store_error("replace cached stage outputs", error))?;
        {
            let mut statement = transaction
                .prepare("INSERT INTO stage_outputs(stage, path, digest) VALUES (?1, ?2, ?3)")
                .map_err(|error| store_error("prepare cached stage outputs", error))?;
            for (path, digest) in paths.iter().zip(content_digests) {
                statement
                    .execute(params![stage, path, digest])
                    .map_err(|error| store_error("record cached stage output", error))?;
            }
        }
        if let Some(previous) = previous.filter(|previous| previous != query_key) {
            let remaining_bindings = transaction
                .query_row(
                    "SELECT COUNT(*) FROM stage_bindings WHERE query_key = ?1",
                    [&previous],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(|error| store_error("count cached stage result owners", error))?;
            if remaining_bindings == 0 {
                transaction
                    .execute("DELETE FROM query_results WHERE query_key = ?1", [previous])
                    .map_err(|error| store_error("retire unbound cached stage result", error))?;
            }
        }
        transaction
            .commit()
            .map_err(|error| store_error("commit cached stage transaction", error))?;
        self.validate_root_identity()
    }

    pub(crate) fn restore_output(&self, digest: &str, destination: &Path) -> Result<()> {
        self.validate_root_identity()?;
        let (mut pack, length) = self.open_object(digest)?;
        let parent = destination.parent().unwrap_or_else(|| Path::new("."));
        fs::create_dir_all(parent)?;
        let name = destination
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("output");
        let restore_id = NEXT_RESTORE_ID.fetch_add(1, Ordering::Relaxed);
        let temporary = parent.join(format!(
            ".{name}.blobray-restore-{}-{restore_id}",
            std::process::id()
        ));
        let result = (|| -> Result<()> {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let mut remaining = length;
            let mut hasher = Sha256::new();
            let mut buffer = [0_u8; 64 * 1024];
            while remaining != 0 {
                let requested = usize::try_from(remaining.min(buffer.len() as u64))
                    .expect("bounded restore read");
                pack.read_exact(&mut buffer[..requested])?;
                output.write_all(&buffer[..requested])?;
                hasher.update(&buffer[..requested]);
                remaining -= requested as u64;
            }
            if format!("{:x}", hasher.finalize()) != digest {
                return Err(crate::Error::invalid(format!(
                    "query cache object {digest} failed its content digest"
                )));
            }
            output.flush()?;
            output.sync_data()?;
            fs::rename(&temporary, destination)?;
            Ok(())
        })();
        if result.is_err() && temporary.is_file() {
            let _ = fs::remove_file(temporary);
        }
        result
    }

    fn validate_object_payload(&self, digest: &str) -> Result<()> {
        let (mut pack, mut remaining) = self.open_object(digest)?;
        let mut hasher = Sha256::new();
        let mut buffer = [0_u8; 64 * 1024];
        while remaining != 0 {
            let requested = usize::try_from(remaining.min(buffer.len() as u64))
                .expect("bounded cache validation read");
            pack.read_exact(&mut buffer[..requested])?;
            hasher.update(&buffer[..requested]);
            remaining -= requested as u64;
        }
        if format!("{:x}", hasher.finalize()) != digest {
            return Err(crate::Error::invalid(format!(
                "query cache object {digest} failed its content digest"
            )));
        }
        Ok(())
    }

    /// Store one completed immutable query value and its direct query edges.
    /// Query callers compute `query_key` from the semantic revision and exact
    /// input fingerprints; scopes and profile names are not valid inputs.
    pub(crate) fn put(
        &mut self,
        query_key: &str,
        kind: &str,
        input_fingerprint: &str,
        dependencies: &[String],
        value: &[u8],
    ) -> Result<String> {
        self.validate_root_identity()?;
        let result_digest = sha256_hex(value);
        let mut requested_dependencies = dependencies.to_vec();
        requested_dependencies.sort();
        requested_dependencies.dedup();
        let existing = self
            .connection
            .query_row(
                "SELECT kind, input_fingerprint, result_digest
                 FROM query_results WHERE query_key = ?1",
                [query_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| store_error("look up immutable query result", error))?;
        if let Some((existing_kind, existing_inputs, existing_result)) = existing {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT dependency_key FROM query_dependencies
                     WHERE query_key = ?1 ORDER BY dependency_key",
                )
                .map_err(|error| store_error("prepare cached query dependencies", error))?;
            let rows = statement
                .query_map([query_key], |row| row.get::<_, String>(0))
                .map_err(|error| store_error("read cached query dependencies", error))?;
            let mut existing_dependencies = Vec::new();
            for row in rows {
                existing_dependencies.push(
                    row.map_err(|error| store_error("decode cached query dependency", error))?,
                );
            }
            if existing_kind == kind
                && existing_inputs == input_fingerprint
                && existing_result == result_digest
                && existing_dependencies == requested_dependencies
            {
                return Ok(result_digest);
            }
            return Err(crate::Error::invalid(format!(
                "query key {query_key:?} was reused for a different immutable result"
            )));
        }
        let (inline_value, object_digest) = if value.len() <= INLINE_VALUE_LIMIT {
            (Some(value), None)
        } else {
            self.ensure_object(&result_digest, value)?;
            (None, Some(result_digest.as_str()))
        };
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| store_error("begin query-result transaction", error))?;
        transaction
            .execute(
                "INSERT INTO query_results(
                     query_key, kind, input_fingerprint, result_digest, inline_value, object_digest
                 ) VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ",
                params![
                    query_key,
                    kind,
                    input_fingerprint,
                    result_digest,
                    inline_value,
                    object_digest
                ],
            )
            .map_err(|error| store_error("record query result", error))?;
        transaction
            .execute(
                "DELETE FROM query_dependencies WHERE query_key = ?1",
                [query_key],
            )
            .map_err(|error| store_error("replace query dependencies", error))?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO query_dependencies(query_key, dependency_key) VALUES (?1, ?2)",
                )
                .map_err(|error| store_error("prepare query dependencies", error))?;
            for dependency in &requested_dependencies {
                statement
                    .execute(params![query_key, dependency])
                    .map_err(|error| store_error("record query dependency", error))?;
            }
        }
        transaction
            .commit()
            .map_err(|error| store_error("commit query-result transaction", error))?;
        self.validate_root_identity()?;
        Ok(result_digest)
    }

    pub(crate) fn get(&self, query_key: &str) -> Result<Option<Vec<u8>>> {
        self.validate_root_identity()?;
        let location = self
            .connection
            .query_row(
                "SELECT result_digest, inline_value, object_digest
                 FROM query_results WHERE query_key = ?1",
                [query_key],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, Option<Vec<u8>>>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| store_error("read query result", error))?;
        let Some((expected_digest, inline, object)) = location else {
            return Ok(None);
        };
        let value = match (inline, object) {
            (Some(value), None) => value,
            (None, Some(digest)) => self.read_object(&digest)?,
            _ => {
                return Err(crate::Error::invalid(format!(
                    "query cache entry {query_key:?} has an invalid value location"
                )));
            }
        };
        if sha256_hex(&value) != expected_digest {
            return Err(crate::Error::invalid(format!(
                "query cache entry {query_key:?} failed its content digest"
            )));
        }
        self.validate_root_identity()?;
        Ok(Some(value))
    }

    /// Store many small immutable function facts in one SQLite transaction.
    ///
    /// Function workers never touch SQLite. The analysis layer collects misses
    /// in memory and publishes them here after the parallel phase, avoiding a
    /// transaction/fsync boundary for every analyzed symbol.
    fn put_function_fact_batch(&mut self, facts: &[(String, Vec<u8>)]) -> Result<()> {
        self.put_function_fact_batch_observed(facts, |_| {})
    }

    fn put_function_fact_batch_observed(
        &mut self,
        facts: &[(String, Vec<u8>)],
        observe_statement: impl FnMut(usize),
    ) -> Result<()> {
        self.validate_root_identity()?;
        // Canonicalize the complete caller batch before the first SQLite or
        // pack access. Stable key order makes statement boundaries independent
        // of worker completion order. Exact duplicates coalesce; conflicting
        // duplicates fail before any persistent state can change.
        let mut canonical: BTreeMap<&str, &[u8]> = BTreeMap::new();
        let mut conflicting_keys = BTreeSet::new();
        for (query_key, value) in facts {
            if let Some(existing) = canonical.get(query_key.as_str()) {
                if *existing != value.as_slice() {
                    conflicting_keys.insert(query_key.as_str());
                }
                continue;
            }
            canonical.insert(query_key, value);
        }
        if let Some(query_key) = conflicting_keys.first() {
            return Err(crate::Error::invalid(format!(
                "function fact key {query_key:?} was reused for a different immutable result within one batch"
            )));
        }
        let prepared = canonical
            .into_iter()
            .map(|(query_key, value)| PreparedFunctionFact {
                query_key,
                value,
                result_digest: sha256_hex(value),
            })
            .collect::<Vec<_>>();
        let inserts = self.preflight_function_fact_batches(&prepared, observe_statement)?;
        if inserts.is_empty() {
            return Ok(());
        }
        self.ensure_objects(inserts.iter().filter_map(|index| {
            let fact = &prepared[*index];
            (fact.value.len() > INLINE_VALUE_LIMIT)
                .then_some((fact.result_digest.as_str(), fact.value))
        }))?;
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| store_error("begin function-fact transaction", error))?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT INTO query_results(
                         query_key, kind, input_fingerprint, result_digest, inline_value, object_digest
                     ) VALUES (?1, 'function-direct-fact', ?1, ?2, ?3, ?4)",
                )
                .map_err(|error| store_error("prepare function-fact insert", error))?;
            for index in inserts {
                let fact = &prepared[index];
                let (inline_value, object_digest) = if fact.value.len() <= INLINE_VALUE_LIMIT {
                    (Some(fact.value), None)
                } else {
                    (None, Some(fact.result_digest.as_str()))
                };
                statement
                    .execute(params![
                        fact.query_key,
                        fact.result_digest,
                        inline_value,
                        object_digest
                    ])
                    .map_err(|error| store_error("record function fact", error))?;
            }
        }
        transaction
            .commit()
            .map_err(|error| store_error("commit function-fact transaction", error))?;
        self.validate_root_identity()
    }

    fn preflight_function_fact_batches(
        &self,
        facts: &[PreparedFunctionFact<'_>],
        mut observe_statement: impl FnMut(usize),
    ) -> Result<Vec<usize>> {
        let full_key_count = facts.len() / FUNCTION_FACT_LOOKUP_BATCH * FUNCTION_FACT_LOOKUP_BATCH;
        let (full_batches, tail) = facts.split_at(full_key_count);
        let mut inserts = Vec::new();

        if !full_batches.is_empty() {
            let sql = function_fact_preflight_sql(FUNCTION_FACT_LOOKUP_BATCH);
            let mut statement = self
                .connection
                .prepare(&sql)
                .map_err(|error| store_error("prepare function-fact preflight", error))?;
            for (batch_index, batch) in full_batches
                .chunks_exact(FUNCTION_FACT_LOOKUP_BATCH)
                .enumerate()
            {
                observe_statement(batch.len());
                append_function_fact_preflight_batch(
                    &mut statement,
                    batch,
                    batch_index * FUNCTION_FACT_LOOKUP_BATCH,
                    &mut inserts,
                )?;
            }
        }
        if !tail.is_empty() {
            let sql = function_fact_preflight_sql(tail.len());
            let mut statement = self
                .connection
                .prepare(&sql)
                .map_err(|error| store_error("prepare final function-fact preflight", error))?;
            observe_statement(tail.len());
            append_function_fact_preflight_batch(
                &mut statement,
                tail,
                full_key_count,
                &mut inserts,
            )?;
        }
        Ok(inserts)
    }

    fn ensure_object(&mut self, digest: &str, value: &[u8]) -> Result<()> {
        self.ensure_objects(std::iter::once((digest, value)))
    }

    fn ensure_file_objects(&mut self, outputs: &[(String, String, PathBuf)]) -> Result<()> {
        let mut missing = Vec::new();
        let mut scheduled = BTreeSet::new();
        for (_, digest, path) in outputs {
            if !scheduled.insert(digest.clone()) {
                continue;
            }
            let existing = self
                .connection
                .query_row("SELECT 1 FROM objects WHERE digest = ?1", [digest], |_| {
                    Ok(())
                })
                .optional()
                .map_err(|error| store_error("look up cached output object", error))?;
            if existing.is_none() {
                missing.push((digest.clone(), path.clone()));
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let storage_pack_path = self.active_storage_pack_path()?;
        let mut pack = open_pack_for_append(
            &self.root,
            &self.root_identity,
            &self.storage_root,
            &self.pack_path,
        )?;
        let mut locations = Vec::with_capacity(missing.len());
        for (digest, path) in missing {
            let mut input = File::open(&path)?;
            let length = input.metadata()?.len();
            let offset = pack.seek(SeekFrom::End(0))?;
            pack.write_all(PACK_RECORD_MAGIC)?;
            pack.write_all(&hex_digest(&digest)?)?;
            pack.write_all(&length.to_le_bytes())?;
            let mut copied = 0_u64;
            let mut hasher = Sha256::new();
            let mut buffer = [0_u8; 64 * 1024];
            loop {
                let read = input.read(&mut buffer)?;
                if read == 0 {
                    break;
                }
                pack.write_all(&buffer[..read])?;
                hasher.update(&buffer[..read]);
                copied += read as u64;
            }
            if copied != length || format!("{:x}", hasher.finalize()) != digest {
                return Err(crate::Error::invalid(format!(
                    "generated output {} changed while it was entering the query cache",
                    path.display()
                )));
            }
            locations.push((digest, offset, length));
        }
        pack.flush()?;
        pack.sync_data()?;
        verify_open_file_path(&pack, &storage_pack_path, "query cache active pack")?;
        self.validate_root_identity()?;
        self.index_objects(locations)
    }

    /// Append a batch before publishing any SQLite location. One durability
    /// barrier covers the whole batch; analysis workers never write SQLite or
    /// fsync once per function/output.
    fn ensure_objects<'a>(
        &mut self,
        values: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) -> Result<()> {
        self.ensure_objects_after_pack_sync(values, || {})
    }

    fn ensure_objects_after_pack_sync<'a>(
        &mut self,
        values: impl IntoIterator<Item = (&'a str, &'a [u8])>,
        after_pack_sync: impl FnOnce(),
    ) -> Result<()> {
        let mut missing = Vec::new();
        let mut scheduled = BTreeSet::new();
        for (digest, value) in values {
            if !scheduled.insert(digest.to_owned()) {
                continue;
            }
            let existing = self
                .connection
                .query_row("SELECT 1 FROM objects WHERE digest = ?1", [digest], |_| {
                    Ok(())
                })
                .optional()
                .map_err(|error| store_error("look up cached object", error))?;
            if existing.is_none() {
                missing.push((digest.to_owned(), value));
            }
        }
        if missing.is_empty() {
            return Ok(());
        }
        let storage_pack_path = self.active_storage_pack_path()?;
        let mut pack = open_pack_for_append(
            &self.root,
            &self.root_identity,
            &self.storage_root,
            &self.pack_path,
        )?;
        let mut locations = Vec::with_capacity(missing.len());
        for (digest, value) in missing {
            let raw_digest = hex_digest(&digest)?;
            let offset = pack.seek(SeekFrom::End(0))?;
            pack.write_all(PACK_RECORD_MAGIC)?;
            pack.write_all(&raw_digest)?;
            pack.write_all(&(value.len() as u64).to_le_bytes())?;
            pack.write_all(value)?;
            locations.push((digest, offset, value.len() as u64));
        }
        pack.flush()?;
        pack.sync_data()?;
        after_pack_sync();
        verify_open_file_path(&pack, &storage_pack_path, "query cache active pack")?;
        self.validate_root_identity()?;
        self.index_objects(locations)
    }

    fn index_objects(&mut self, locations: PackedObjectLocations) -> Result<()> {
        self.validate_root_identity()?;
        let pack_name = self
            .pack_path
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| crate::Error::invalid("query cache pack has no UTF-8 file name"))?
            .to_owned();
        let transaction = self
            .connection
            .transaction()
            .map_err(|error| store_error("begin cached-object transaction", error))?;
        {
            let mut statement = transaction
                .prepare(
                    "INSERT OR IGNORE INTO objects(digest, pack_name, pack_offset, payload_length)
                     VALUES (?1, ?2, ?3, ?4)",
                )
                .map_err(|error| store_error("prepare cached objects", error))?;
            for (digest, offset, length) in locations {
                statement
                    .execute(params![digest, &pack_name, offset as i64, length as i64])
                    .map_err(|error| store_error("index cached object", error))?;
            }
        }
        transaction
            .commit()
            .map_err(|error| store_error("commit cached objects", error))?;
        self.validate_root_identity()
    }

    fn read_object(&self, digest: &str) -> Result<Vec<u8>> {
        let (mut pack, length) = self.open_object(digest)?;
        let mut value = vec![
            0_u8;
            usize::try_from(length).map_err(|_| {
                crate::Error::invalid(format!("query cache object {digest} is too large"))
            })?
        ];
        pack.read_exact(&mut value)?;
        Ok(value)
    }

    fn open_object(&self, digest: &str) -> Result<(File, u64)> {
        self.validate_root_identity()?;
        let (pack_name, offset, length) = self
            .connection
            .query_row(
                "SELECT pack_name, pack_offset, payload_length FROM objects WHERE digest = ?1",
                [digest],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, i64>(2)?,
                    ))
                },
            )
            .optional()
            .map_err(|error| store_error("locate cached object", error))?
            .ok_or_else(|| {
                crate::Error::invalid(format!("query cache object {digest} is missing"))
            })?;
        let offset = u64::try_from(offset).map_err(|_| {
            crate::Error::invalid(format!("query cache object {digest} has a negative offset"))
        })?;
        let length = u64::try_from(length).map_err(|_| {
            crate::Error::invalid(format!("query cache object {digest} has a negative length"))
        })?;
        if !is_pack_name(&pack_name) {
            return Err(crate::Error::invalid(format!(
                "query cache object {digest} references invalid pack name {pack_name:?}"
            )));
        }
        let pack_path = self.root.join(&pack_name);
        let storage_pack_path = self.storage_root.join(&pack_name);
        let metadata = fs::symlink_metadata(&storage_pack_path).map_err(|error| {
            crate::Error::invalid(format!(
                "cannot inspect query cache pack {}: {error}",
                pack_path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(crate::Error::invalid(format!(
                "query cache indexed pack {} is not a regular file",
                pack_path.display()
            )));
        }
        let required = offset
            .checked_add(PACK_HEADER_BYTES)
            .and_then(|end| end.checked_add(length))
            .ok_or_else(|| {
                crate::Error::invalid(format!(
                    "query cache object {digest} has an overflowing pack extent"
                ))
            })?;
        if metadata.len() < required {
            return Err(crate::Error::invalid(format!(
                "query cache pack {} has {} bytes but object {digest} requires {required}",
                pack_path.display(),
                metadata.len()
            )));
        }
        let mut pack = open_cache_file_read_only(&storage_pack_path)?;
        pack.seek(SeekFrom::Start(offset))?;
        let mut magic = [0_u8; 8];
        let mut stored_digest = [0_u8; 32];
        let mut stored_length = [0_u8; 8];
        pack.read_exact(&mut magic)?;
        pack.read_exact(&mut stored_digest)?;
        pack.read_exact(&mut stored_length)?;
        if &magic != PACK_RECORD_MAGIC
            || stored_digest != hex_digest(digest)?
            || u64::from_le_bytes(stored_length) != length
        {
            return Err(crate::Error::invalid(format!(
                "query cache object {digest} has an invalid pack header"
            )));
        }
        pack.seek(SeekFrom::Start(offset + PACK_HEADER_BYTES))?;
        Ok((pack, length))
    }

    #[cfg(not(target_os = "linux"))]
    fn compact_if_needed(&mut self) -> Result<()> {
        self.validate_root_identity()
    }

    #[cfg(target_os = "linux")]
    fn compact_if_needed(&mut self) -> Result<()> {
        self.validate_root_identity()?;
        let total_pack_bytes = pack_files(&self.storage_root)?
            .into_iter()
            .try_fold(0_u64, |total, path| -> Result<u64> {
                Ok(total.saturating_add(fs::metadata(path)?.len()))
            })?;
        if total_pack_bytes < COMPACT_MIN_PACK_BYTES {
            return self.validate_root_identity();
        }
        let live_record_bytes = self
            .connection
            .query_row(
                "SELECT COALESCE(SUM(?1 + payload_length), 0)
                 FROM objects
                 WHERE digest IN (
                     SELECT object_digest FROM query_results WHERE object_digest IS NOT NULL
                     UNION
                     SELECT digest FROM stage_outputs
                 )",
                [PACK_HEADER_BYTES as i64],
                |row| row.get::<_, i64>(0),
            )
            .map_err(|error| store_error("measure live query-cache objects", error))?;
        let live_record_bytes = u64::try_from(live_record_bytes).map_err(|_| {
            crate::Error::invalid("query cache reported a negative live-object size")
        })?;
        let reclaimable = total_pack_bytes.saturating_sub(live_record_bytes);
        if reclaimable < COMPACT_MIN_RECLAIMABLE_BYTES
            || reclaimable.saturating_mul(100)
                < total_pack_bytes.saturating_mul(u64::from(COMPACT_MIN_RECLAIMABLE_PERCENT))
        {
            return self.validate_root_identity();
        }
        tracing::info!(
            total_pack_bytes,
            live_record_bytes,
            reclaimable_bytes = reclaimable,
            "compacting persistent query CAS"
        );
        self.compact()
    }

    /// Copy every reachable object into a new immutable pack generation,
    /// atomically redirect SQLite, then remove packs no longer referenced by
    /// the index. A crash before the transaction leaves an orphan new pack;
    /// a crash after it leaves an orphan old pack. Both are removed on open.
    #[cfg(not(target_os = "linux"))]
    fn compact(&mut self) -> Result<()> {
        self.validate_root_identity()
    }

    #[cfg(target_os = "linux")]
    fn compact(&mut self) -> Result<()> {
        self.validate_root_identity()?;
        let live = {
            let mut statement = self
                .connection
                .prepare(
                    "SELECT digest FROM objects
                     WHERE digest IN (
                         SELECT object_digest FROM query_results WHERE object_digest IS NOT NULL
                         UNION
                         SELECT digest FROM stage_outputs
                     )
                     ORDER BY digest",
                )
                .map_err(|error| store_error("prepare live query-cache objects", error))?;
            let rows = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| store_error("read live query-cache objects", error))?;
            let mut live = Vec::new();
            for row in rows {
                live.push(row.map_err(|error| {
                    store_error("decode live query-cache object digest", error)
                })?);
            }
            live
        };

        let pack_name = format!("objects-{}.pack", self.next_pack_generation);
        let destination = self.storage_root.join(&pack_name);
        let temporary = self.storage_root.join(format!(
            ".{pack_name}.compact-{}-{}",
            std::process::id(),
            NEXT_RESTORE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let write_result = (|| -> Result<(PackedObjectLocations, File)> {
            let mut output = OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&temporary)?;
            let mut locations = Vec::with_capacity(live.len());
            for digest in &live {
                let (mut input, length) = self.open_object(digest)?;
                let offset = output.stream_position()?;
                output.write_all(PACK_RECORD_MAGIC)?;
                output.write_all(&hex_digest(digest)?)?;
                output.write_all(&length.to_le_bytes())?;
                let mut remaining = length;
                let mut hasher = Sha256::new();
                let mut buffer = [0_u8; 64 * 1024];
                while remaining != 0 {
                    let requested = usize::try_from(remaining.min(buffer.len() as u64))
                        .expect("bounded cache compaction read");
                    input.read_exact(&mut buffer[..requested])?;
                    output.write_all(&buffer[..requested])?;
                    hasher.update(&buffer[..requested]);
                    remaining -= requested as u64;
                }
                if format!("{:x}", hasher.finalize()) != *digest {
                    return Err(crate::Error::invalid(format!(
                        "query cache object {digest} failed its digest during compaction"
                    )));
                }
                locations.push((digest.clone(), offset, length));
            }
            output.flush()?;
            output.sync_data()?;
            Ok((locations, output))
        })();
        let (locations, output) = match write_result {
            Ok(written) => written,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
        fs::rename(&temporary, &destination)?;
        verify_open_file_path(&output, &destination, "query cache compacted pack")?;
        self.validate_root_identity()?;

        let transaction = self
            .connection
            .transaction()
            .map_err(|error| store_error("begin query-cache compaction transaction", error))?;
        {
            let mut statement = transaction
                .prepare(
                    "UPDATE objects
                     SET pack_name = ?2, pack_offset = ?3, payload_length = ?4
                     WHERE digest = ?1",
                )
                .map_err(|error| store_error("prepare compacted object locations", error))?;
            for (digest, offset, length) in &locations {
                statement
                    .execute(params![digest, &pack_name, *offset as i64, *length as i64])
                    .map_err(|error| store_error("record compacted object location", error))?;
            }
        }
        transaction
            .execute(
                "DELETE FROM objects
                 WHERE digest NOT IN (
                     SELECT object_digest FROM query_results WHERE object_digest IS NOT NULL
                     UNION
                     SELECT digest FROM stage_outputs
                 )",
                [],
            )
            .map_err(|error| store_error("delete unreachable query-cache objects", error))?;
        let next_generation = self.next_pack_generation.checked_add(1).ok_or_else(|| {
            crate::Error::invalid("query cache exhausted its pack generation counter")
        })?;
        transaction
            .execute(
                "UPDATE cache_state
                 SET active_pack = ?1, next_pack_generation = ?2
                 WHERE singleton = 1",
                params![&pack_name, next_generation as i64],
            )
            .map_err(|error| store_error("activate compacted query-cache pack", error))?;
        transaction
            .commit()
            .map_err(|error| store_error("commit query-cache compaction", error))?;

        self.pack_path = self.root.join(&pack_name);
        self.next_pack_generation = next_generation;
        self.remove_unreferenced_pack_files()
    }

    #[cfg(not(target_os = "linux"))]
    fn remove_unreferenced_pack_files(&mut self) -> Result<()> {
        self.validate_root_identity()
    }

    #[cfg(target_os = "linux")]
    fn remove_unreferenced_pack_files(&mut self) -> Result<()> {
        self.validate_root_identity()?;
        let mut referenced = BTreeSet::new();
        referenced.insert(
            self.pack_path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| crate::Error::invalid("query cache pack has no UTF-8 file name"))?
                .to_owned(),
        );
        let mut statement = self
            .connection
            .prepare("SELECT DISTINCT pack_name FROM objects")
            .map_err(|error| store_error("prepare referenced query-cache packs", error))?;
        let rows = statement
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|error| store_error("read referenced query-cache packs", error))?;
        for row in rows {
            referenced.insert(
                row.map_err(|error| store_error("decode referenced query-cache pack", error))?,
            );
        }
        drop(statement);
        for path in pack_files(&self.storage_root)? {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| {
                    crate::Error::invalid("query cache contains a non-UTF-8 pack name")
                })?;
            if !referenced.contains(name) {
                fs::remove_file(path)?;
            }
        }
        self.validate_root_identity()
    }

    fn load_function_facts_batched(
        &self,
        keys: &[String],
        mut observe_statement: impl FnMut(usize),
    ) -> Result<Vec<(String, Vec<u8>)>> {
        self.validate_root_identity()?;
        let full_key_count = keys.len() / FUNCTION_FACT_LOOKUP_BATCH * FUNCTION_FACT_LOOKUP_BATCH;
        let (full_batches, tail) = keys.split_at(full_key_count);
        let mut output = Vec::new();

        if !full_batches.is_empty() {
            let sql = function_fact_lookup_sql(FUNCTION_FACT_LOOKUP_BATCH);
            let mut statement = self
                .connection
                .prepare(&sql)
                .map_err(|error| store_error("prepare batched function-fact lookup", error))?;
            for batch in full_batches.chunks_exact(FUNCTION_FACT_LOOKUP_BATCH) {
                observe_statement(batch.len());
                self.append_function_fact_batch(&mut statement, batch, &mut output)?;
            }
        }
        if !tail.is_empty() {
            let sql = function_fact_lookup_sql(tail.len());
            let mut statement = self
                .connection
                .prepare(&sql)
                .map_err(|error| store_error("prepare final function-fact lookup", error))?;
            observe_statement(tail.len());
            self.append_function_fact_batch(&mut statement, tail, &mut output)?;
        }
        self.validate_root_identity()?;
        Ok(output)
    }

    fn append_function_fact_batch(
        &self,
        statement: &mut rusqlite::Statement<'_>,
        keys: &[String],
        output: &mut Vec<(String, Vec<u8>)>,
    ) -> Result<()> {
        // Drop the active SQLite rows before resolving any pack-backed values:
        // read_object performs a second lookup on the same connection. The
        // temporary payload set remains bounded by FUNCTION_FACT_LOOKUP_BATCH.
        let locations = {
            let mut rows = statement
                .query(rusqlite::params_from_iter(keys))
                .map_err(|error| store_error("read function-fact batch", error))?;
            let mut locations = Vec::new();
            while let Some(row) = rows
                .next()
                .map_err(|error| store_error("advance function-fact batch", error))?
            {
                let location = (|| -> rusqlite::Result<_> {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<Vec<u8>>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                })()
                .map_err(|error| store_error("decode function-fact batch", error))?;
                locations.push(location);
            }
            locations
        };
        for (key, expected_digest, inline, object_digest) in locations {
            let value = match (inline, object_digest) {
                (Some(value), None) => value,
                (None, Some(object_digest)) => self.read_object(&object_digest)?,
                _ => {
                    return Err(crate::Error::invalid(format!(
                        "function fact {key:?} has an invalid value location"
                    )));
                }
            };
            if sha256_hex(&value) != expected_digest {
                return Err(crate::Error::invalid(format!(
                    "function fact {key:?} failed its content digest"
                )));
            }
            output.push((key, value));
        }
        Ok(())
    }
}

impl crate::analysis::FunctionFactStore for QueryStore {
    fn load_function_facts(&self, keys: &[String]) -> Result<Vec<(String, Vec<u8>)>> {
        self.load_function_facts_batched(keys, |_| {})
    }

    fn store_function_facts(&mut self, facts: &[(String, Vec<u8>)]) -> Result<()> {
        self.put_function_fact_batch(facts)
    }
}

fn function_fact_preflight_sql(parameter_count: usize) -> String {
    debug_assert!((1..=FUNCTION_FACT_LOOKUP_BATCH).contains(&parameter_count));
    let mut sql = String::from("WITH requested(ordinal, query_key) AS (VALUES ");
    for ordinal in 0..parameter_count {
        if ordinal != 0 {
            sql.push_str(", ");
        }
        std::fmt::Write::write_fmt(&mut sql, format_args!("({ordinal}, ?)"))
            .expect("writing SQL into a String cannot fail");
    }
    sql.push_str(
        ") SELECT requested.ordinal, result.kind, result.input_fingerprint, \
         result.result_digest FROM requested LEFT JOIN query_results AS result \
         ON result.query_key = requested.query_key ORDER BY requested.ordinal",
    );
    sql
}

fn append_function_fact_preflight_batch(
    statement: &mut rusqlite::Statement<'_>,
    facts: &[PreparedFunctionFact<'_>],
    base_index: usize,
    inserts: &mut Vec<usize>,
) -> Result<()> {
    let mut rows = statement
        .query(rusqlite::params_from_iter(
            facts.iter().map(|fact| fact.query_key),
        ))
        .map_err(|error| store_error("read function-fact preflight", error))?;
    let mut position = 0_usize;
    while let Some(row) = rows
        .next()
        .map_err(|error| store_error("advance function-fact preflight", error))?
    {
        let ordinal = row
            .get::<_, i64>(0)
            .map_err(|error| store_error("decode function-fact preflight ordinal", error))?;
        let ordinal = usize::try_from(ordinal)
            .map_err(|_| crate::Error::invalid("function-fact preflight has a negative ordinal"))?;
        if ordinal != position || position >= facts.len() {
            return Err(crate::Error::invalid(
                "function-fact preflight returned an invalid row order",
            ));
        }
        let kind = row
            .get::<_, Option<String>>(1)
            .map_err(|error| store_error("decode function-fact preflight kind", error))?;
        let inputs = row
            .get::<_, Option<String>>(2)
            .map_err(|error| store_error("decode function-fact preflight inputs", error))?;
        let digest = row
            .get::<_, Option<String>>(3)
            .map_err(|error| store_error("decode function-fact preflight digest", error))?;
        let fact = &facts[position];
        match (kind, inputs, digest) {
            (None, None, None) => inserts.push(base_index + position),
            (Some(kind), Some(inputs), Some(digest))
                if kind == "function-direct-fact"
                    && inputs == fact.query_key
                    && digest == fact.result_digest => {}
            _ => {
                return Err(crate::Error::invalid(format!(
                    "function fact key {:?} was reused for a different immutable result",
                    fact.query_key
                )));
            }
        }
        position += 1;
    }
    if position != facts.len() {
        return Err(crate::Error::invalid(format!(
            "function-fact preflight returned {position} rows for {} keys",
            facts.len()
        )));
    }
    Ok(())
}

fn function_fact_lookup_sql(parameter_count: usize) -> String {
    debug_assert!((1..=FUNCTION_FACT_LOOKUP_BATCH).contains(&parameter_count));
    let mut sql = String::from("WITH requested(ordinal, query_key) AS (VALUES ");
    for ordinal in 0..parameter_count {
        if ordinal != 0 {
            sql.push_str(", ");
        }
        std::fmt::Write::write_fmt(&mut sql, format_args!("({ordinal}, ?)"))
            .expect("writing SQL into a String cannot fail");
    }
    sql.push_str(
        ") SELECT requested.query_key, result.result_digest, result.inline_value, \
         result.object_digest FROM requested JOIN query_results AS result \
         ON result.query_key = requested.query_key \
         AND result.kind = 'function-direct-fact' ORDER BY requested.ordinal",
    );
    sql
}

fn fingerprint_cache_tree(root: &Path) -> Result<CacheTreeFingerprint> {
    fn metadata_entry(
        path: PathBuf,
        metadata: &fs::Metadata,
        kind: CacheTreeEntryKind,
    ) -> CacheTreeEntry {
        CacheTreeEntry {
            path,
            kind,
            len: metadata.len(),
            modified: metadata.modified().ok(),
            readonly: metadata.permissions().readonly(),
        }
    }

    fn collect(root: &Path, directory: &Path, entries: &mut Vec<CacheTreeEntry>) -> Result<()> {
        let mut paths = fs::read_dir(directory)?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<std::io::Result<Vec<_>>>()?;
        paths.sort();
        for path in paths {
            let metadata = fs::symlink_metadata(&path)?;
            let relative = path.strip_prefix(root).map_err(|error| {
                crate::Error::invalid(format!(
                    "query cache entry {} is outside {}: {error}",
                    path.display(),
                    root.display()
                ))
            })?;
            if metadata.file_type().is_symlink() {
                return Err(crate::Error::invalid(format!(
                    "query cache contains symbolic link {}",
                    path.display()
                )));
            }
            if metadata.is_dir() {
                entries.push(metadata_entry(
                    relative.to_owned(),
                    &metadata,
                    CacheTreeEntryKind::Directory,
                ));
                collect(root, &path, entries)?;
            } else if metadata.is_file() {
                entries.push(metadata_entry(
                    relative.to_owned(),
                    &metadata,
                    CacheTreeEntryKind::File,
                ));
            } else {
                return Err(crate::Error::invalid(format!(
                    "query cache contains unsupported filesystem entry {}",
                    path.display()
                )));
            }
        }
        Ok(())
    }

    let metadata = fs::symlink_metadata(root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(crate::Error::invalid(format!(
            "query cache root {} is not a regular directory",
            root.display()
        )));
    }
    let mut entries = vec![metadata_entry(
        PathBuf::new(),
        &metadata,
        CacheTreeEntryKind::Directory,
    )];
    collect(root, root, &mut entries)?;
    Ok(CacheTreeFingerprint { entries })
}

fn reject_nonempty_wal(fingerprint: &CacheTreeFingerprint, database_path: &Path) -> Result<()> {
    if fingerprint.has_nonempty_wal() {
        return Err(crate::Error::invalid(format!(
            "query cache has an active SQLite WAL {}; retry after the cache writer exits",
            sqlite_sidecar_path(database_path, "-wal").display()
        )));
    }
    Ok(())
}

fn immutable_database_uri_pinned(database: &Path) -> Result<String> {
    #[cfg(target_os = "linux")]
    let database = database.to_owned();
    #[cfg(not(target_os = "linux"))]
    let database = fs::canonicalize(database)?;
    immutable_database_uri_for_path(&database)
}

fn immutable_database_uri_for_path(database: &Path) -> Result<String> {
    let database = database.to_str().ok_or_else(|| {
        crate::Error::invalid(format!(
            "query cache database path {} is not UTF-8",
            database.display()
        ))
    })?;
    let mut uri = String::from("file:");
    for byte in database.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'/' | b'-' | b'_' | b'.' | b'~') {
            uri.push(char::from(byte));
        } else {
            std::fmt::Write::write_fmt(&mut uri, format_args!("%{byte:02X}"))
                .expect("writing a URI into a String cannot fail");
        }
    }
    uri.push_str("?mode=ro&immutable=1");
    Ok(uri)
}

fn sqlite_sidecar_path(database: &Path, suffix: &str) -> PathBuf {
    let mut path = database.as_os_str().to_os_string();
    path.push(suffix);
    path.into()
}

fn nonnegative(value: i64, label: &str) -> Result<u64> {
    u64::try_from(value)
        .map_err(|_| crate::Error::invalid(format!("query cache reported a negative {label}")))
}

fn query_nonnegative_count(connection: &Connection, context: &str, sql: &str) -> Result<u64> {
    let value = connection
        .query_row(sql, [], |row| row.get::<_, i64>(0))
        .map_err(|error| store_error(context, error))?;
    nonnegative(value, context)
}

fn query_nonnegative_pair(connection: &Connection, context: &str, sql: &str) -> Result<(u64, u64)> {
    let (first, second) = connection
        .query_row(sql, [], |row| {
            Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|error| store_error(context, error))?;
    Ok((nonnegative(first, context)?, nonnegative(second, context)?))
}

fn validate_indexed_pack_extents(connection: &Connection, root: &Path) -> Result<()> {
    let mut statement = connection
        .prepare(
            "SELECT pack_name, MAX(pack_offset + ?1 + payload_length)
             FROM objects GROUP BY pack_name ORDER BY pack_name",
        )
        .map_err(|error| store_error("prepare indexed pack extents", error))?;
    let mut rows = statement
        .query([PACK_HEADER_BYTES as i64])
        .map_err(|error| store_error("read indexed pack extents", error))?;
    while let Some(row) = rows
        .next()
        .map_err(|error| store_error("advance indexed pack extents", error))?
    {
        let name = row
            .get::<_, String>(0)
            .map_err(|error| store_error("decode indexed pack name", error))?;
        let required = row
            .get::<_, i64>(1)
            .map_err(|error| store_error("decode indexed pack extent", error))?;
        let required = nonnegative(required, "indexed pack extent")?;
        if !is_pack_name(&name) {
            return Err(crate::Error::invalid(format!(
                "query cache has an invalid indexed pack name {name:?}"
            )));
        }
        let path = root.join(&name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            crate::Error::invalid(format!(
                "query cache cannot read indexed pack {}: {error}",
                path.display()
            ))
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(crate::Error::invalid(format!(
                "query cache indexed pack {} is not a regular file",
                path.display()
            )));
        }
        if metadata.len() < required {
            return Err(crate::Error::invalid(format!(
                "query cache pack {} has {} bytes but its index requires {required}",
                path.display(),
                metadata.len()
            )));
        }
    }
    Ok(())
}

fn is_pack_name(name: &str) -> bool {
    let Some(generation) = name
        .strip_prefix("objects-")
        .and_then(|name| name.strip_suffix(".pack"))
    else {
        return false;
    };
    !generation.is_empty() && generation.bytes().all(|byte| byte.is_ascii_digit())
}

fn ensure_cache_root(root: &Path) -> Result<()> {
    let generated = root
        .parent()
        .ok_or_else(|| crate::Error::invalid("query cache root has no parent directory"))?;
    ensure_regular_directory(generated, "query cache parent")?;
    ensure_regular_directory(root, "query cache root")
}

fn validate_absent_cache_root_parent(root: &Path) -> Result<()> {
    let generated = root
        .parent()
        .ok_or_else(|| crate::Error::invalid("query cache root has no parent directory"))?;
    match fs::symlink_metadata(generated) {
        Ok(_) => validate_existing_directory(generated, "query cache parent"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let project_root = generated.parent().ok_or_else(|| {
                crate::Error::invalid("query cache parent has no project directory")
            })?;
            validate_existing_directory(project_root, "query cache project root")
        }
        Err(error) => Err(error.into()),
    }
}

fn validate_cold_cache_root(storage_root: &Path) -> Result<()> {
    validate_sqlite_sidecars(&storage_root.join("queries.sqlite3"))?;
    let _ = fingerprint_cache_tree(storage_root)?;
    let _ = pack_files(storage_root)?;
    Ok(())
}

impl CacheRootIdentity {
    fn capture(root: &Path) -> Result<Self> {
        let parent = root
            .parent()
            .ok_or_else(|| crate::Error::invalid("query cache root has no parent directory"))?;
        Ok(Self {
            parent: DirectoryIdentity::capture(parent, "query cache parent")?,
            root: DirectoryIdentity::capture(root, "query cache root")?,
        })
    }

    fn validate(&self, root: &Path) -> Result<()> {
        let current = Self::capture(root)?;
        if current != *self {
            return Err(crate::Error::invalid(format!(
                "query cache root {} was replaced while the store was open",
                root.display()
            )));
        }
        Ok(())
    }
}

impl PlanReadGuard {
    fn validate_lexical_root(&self) -> Result<()> {
        self.root_identity.validate(&self.root)?;
        verify_open_file_path(
            &self.access_lock,
            &self.pinned_root.storage_root.join("queries.sqlite3"),
            "query cache database",
        )?;
        self.root_identity.validate(&self.root)
    }
}

impl PinnedCacheRoot {
    fn open(root: &Path, identity: &CacheRootIdentity) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::{fd::AsRawFd, unix::fs::OpenOptionsExt};

            let mut options = OpenOptions::new();
            options
                .read(true)
                .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC);
            let directory = options.open(root)?;
            let metadata = directory.metadata()?;
            use std::os::unix::fs::MetadataExt;
            if !metadata.is_dir()
                || metadata.dev() != identity.root.device
                || metadata.ino() != identity.root.inode
            {
                return Err(crate::Error::invalid(format!(
                    "query cache root {} changed while it was being pinned",
                    root.display()
                )));
            }
            identity.validate(root)?;
            let storage_root = PathBuf::from(format!("/proc/self/fd/{}/.", directory.as_raw_fd()));
            let pinned_metadata = fs::metadata(&storage_root)?;
            if pinned_metadata.dev() != metadata.dev() || pinned_metadata.ino() != metadata.ino() {
                return Err(crate::Error::invalid(format!(
                    "query cache root {} could not be pinned through /proc/self/fd",
                    root.display()
                )));
            }
            Ok(Self {
                storage_root,
                directory,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            identity.validate(root)?;
            Ok(Self {
                storage_root: root.to_owned(),
            })
        }
    }

    fn try_clone(&self) -> Result<Self> {
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;

            let directory = self.directory.try_clone()?;
            let storage_root = PathBuf::from(format!("/proc/self/fd/{}/.", directory.as_raw_fd()));
            Ok(Self {
                storage_root,
                directory,
            })
        }
        #[cfg(not(target_os = "linux"))]
        {
            Ok(Self {
                storage_root: self.storage_root.clone(),
            })
        }
    }
}

impl PinnedConnection {
    fn read_only(connection: Connection) -> Self {
        Self {
            connection: Some(connection),
            cleanup: None,
        }
    }

    fn writer(
        connection: Connection,
        root: PathBuf,
        root_identity: CacheRootIdentity,
        storage_database_path: PathBuf,
        database_file: File,
    ) -> Result<Self> {
        connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .map_err(|error| store_error("disable SQLite checkpoint-on-close", error))?;
        Ok(Self {
            connection: Some(connection),
            cleanup: Some(PinnedConnectionCleanup {
                root,
                root_identity,
                storage_database_path,
                database_file,
            }),
        })
    }
}

impl Deref for PinnedConnection {
    type Target = Connection;

    fn deref(&self) -> &Self::Target {
        self.connection
            .as_ref()
            .expect("pinned SQLite connection is available before drop")
    }
}

impl DerefMut for PinnedConnection {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.connection
            .as_mut()
            .expect("pinned SQLite connection is available before drop")
    }
}

impl Drop for PinnedConnection {
    fn drop(&mut self) {
        let Some(connection) = self.connection.take() else {
            return;
        };
        let cleanup = self.cleanup.as_ref();
        let storage_binding_is_current = cleanup.is_some_and(|cleanup| {
            cleanup.root_identity.validate(&cleanup.root).is_ok()
                && verify_open_file_path(
                    &cleanup.database_file,
                    &cleanup.storage_database_path,
                    "query cache database",
                )
                .is_ok()
        });
        let checkpointed = !storage_binding_is_current
            || connection
                .execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")
                .is_ok();
        let closed = match connection.close() {
            Ok(()) => true,
            Err((connection, _)) => {
                // NO_CKPT_ON_CLOSE remains set on this fallback drop.
                drop(connection);
                false
            }
        };
        if !storage_binding_is_current || !checkpointed || !closed {
            return;
        }
        let cleanup = cleanup.expect("current storage binding requires writer cleanup state");
        for suffix in ["-wal", "-shm", "-journal"] {
            let path = sqlite_sidecar_path(&cleanup.storage_database_path, suffix);
            match fs::symlink_metadata(&path) {
                Ok(metadata) if metadata.is_file() || metadata.file_type().is_symlink() => {
                    let _ = fs::remove_file(path);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => {}
            }
        }
    }
}

impl DirectoryIdentity {
    fn capture(path: &Path, kind: &str) -> Result<Self> {
        let metadata = fs::symlink_metadata(path)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(crate::Error::invalid(format!(
                "{kind} {} is not a regular directory",
                path.display()
            )));
        }
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Ok(Self {
            canonical: fs::canonicalize(path)?,
            #[cfg(unix)]
            device: metadata.dev(),
            #[cfg(unix)]
            inode: metadata.ino(),
            #[cfg(not(unix))]
            created: metadata.created().ok(),
        })
    }
}

fn ensure_regular_directory(path: &Path, kind: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_dir() {
                return Err(crate::Error::invalid(format!(
                    "{kind} {} is not a regular directory",
                    path.display()
                )));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            fs::create_dir(path)?;
        }
        Err(error) => return Err(error.into()),
    }
    validate_existing_directory(path, kind)
}

fn validate_existing_directory(path: &Path, kind: &str) -> Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(crate::Error::invalid(format!(
            "{kind} {} is not a regular directory",
            path.display()
        )));
    }
    Ok(())
}

fn open_cache_database(path: &Path) -> Result<File> {
    reject_existing_symlink_or_non_file(path, "query cache database")?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    set_no_follow(&mut options);
    let file = options.open(path)?;
    verify_open_file_path(&file, path, "query cache database")?;
    Ok(file)
}

fn open_cache_database_read_only(path: &Path) -> Result<File> {
    open_cache_file_read_only(path)
}

fn open_cache_file_read_only(path: &Path) -> Result<File> {
    reject_existing_symlink_or_non_file(path, "query cache file")?;
    let mut options = OpenOptions::new();
    options.read(true);
    set_no_follow(&mut options);
    let file = options.open(path)?;
    verify_open_file_path(&file, path, "query cache file")?;
    Ok(file)
}

fn validate_sqlite_sidecars(database: &Path) -> Result<()> {
    for suffix in ["-wal", "-shm", "-journal"] {
        let path = sqlite_sidecar_path(database, suffix);
        reject_existing_symlink_or_non_file(&path, "query cache SQLite sidecar")?;
    }
    Ok(())
}

fn open_pack_for_append(
    root: &Path,
    identity: &CacheRootIdentity,
    storage_root: &Path,
    path: &Path,
) -> Result<File> {
    identity.validate(root)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| crate::Error::invalid("query cache pack has no UTF-8 file name"))?;
    if path.parent() != Some(root) || !is_pack_name(name) {
        return Err(crate::Error::invalid(format!(
            "query cache has invalid active pack path {}",
            path.display()
        )));
    }
    let storage_path = storage_root.join(name);
    reject_existing_symlink_or_non_file(&storage_path, "query cache active pack")?;
    let mut options = OpenOptions::new();
    options.create(true).append(true).read(true);
    set_no_follow(&mut options);
    let file = options.open(&storage_path)?;
    verify_open_file_path(&file, &storage_path, "query cache active pack")?;
    identity.validate(root)?;
    Ok(file)
}

fn reject_existing_symlink_or_non_file(path: &Path, kind: &str) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            crate::Error::invalid(format!("{kind} {} is not a regular file", path.display())),
        ),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.into()),
    }
}

fn verify_open_file_path(file: &File, path: &Path, kind: &str) -> Result<()> {
    let opened = file.metadata()?;
    let current = fs::symlink_metadata(path)?;
    if !opened.is_file() || current.file_type().is_symlink() || !current.is_file() {
        return Err(crate::Error::invalid(format!(
            "{kind} {} is not a regular file",
            path.display()
        )));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if opened.dev() != current.dev() || opened.ino() != current.ino() {
            return Err(crate::Error::invalid(format!(
                "{kind} {} changed while it was being opened",
                path.display()
            )));
        }
    }
    Ok(())
}

fn set_no_follow(options: &mut OpenOptions) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC);
    }
    #[cfg(not(unix))]
    let _ = options;
}

fn pack_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if !name.starts_with("objects-") || !name.ends_with(".pack") {
            continue;
        }
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() {
            return Err(crate::Error::invalid(format!(
                "query cache contains symbolic link {}",
                path.display()
            )));
        }
        if metadata.is_file() {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

#[cfg(target_os = "linux")]
fn remove_pack_files(root: &Path) -> Result<()> {
    for path in pack_files(root)? {
        fs::remove_file(path)?;
    }
    let legacy = root.join("objects.pack");
    if legacy.is_file() {
        fs::remove_file(legacy)?;
    }
    Ok(())
}

fn sha256_hex(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}

fn hex_digest(value: &str) -> Result<[u8; 32]> {
    if value.len() != 64 {
        return Err(crate::Error::invalid(format!(
            "invalid SHA-256 digest in query cache: {value:?}"
        )));
    }
    let mut output = [0_u8; 32];
    for (index, byte) in output.iter_mut().enumerate() {
        *byte = u8::from_str_radix(&value[index * 2..index * 2 + 2], 16).map_err(|_| {
            crate::Error::invalid(format!("invalid SHA-256 digest in query cache: {value:?}"))
        })?;
    }
    Ok(output)
}

fn cache_lock_error(access: &str, database: &Path, error: std::fs::TryLockError) -> crate::Error {
    match error {
        std::fs::TryLockError::WouldBlock => crate::Error::invalid(format!(
            "query cache database {} has an active writer or reader; cannot acquire {access} snapshot, retry after the other Blobray process exits",
            database.display()
        )),
        std::fs::TryLockError::Error(error) => crate::Error::invalid(format!(
            "cannot lock query cache database {} for {access}: {error}",
            database.display()
        )),
    }
}

fn store_error(context: &str, error: rusqlite::Error) -> crate::Error {
    crate::Error::invalid(format!("{context}: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("blobray-query-store-{}-{name}", std::process::id()));
        if directory.is_dir() {
            fs::remove_dir_all(&directory).unwrap();
        }
        fs::create_dir_all(&directory).unwrap();
        directory.join("vendor-project.toml")
    }

    fn output(manifest: &Path, name: &str, value: &[u8]) -> (String, String, PathBuf) {
        let path = manifest.parent().unwrap().join(name);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, value).unwrap();
        (path.to_string_lossy().into_owned(), sha256_hex(value), path)
    }

    fn read_only_stage_digests(
        manifest: &Path,
        query_key: &str,
        validate_payloads: bool,
    ) -> Result<Option<Vec<String>>> {
        let Some(guard) = QueryStore::plan_read_guard(manifest)? else {
            return Ok(None);
        };
        QueryStore::stage_output_digests_read_only(&guard, query_key, validate_payloads)
    }

    fn snapshot_tree(root: &Path) -> Vec<(PathBuf, Option<(u64, String)>)> {
        fn collect(
            root: &Path,
            directory: &Path,
            output: &mut Vec<(PathBuf, Option<(u64, String)>)>,
        ) {
            let mut paths = fs::read_dir(directory)
                .unwrap()
                .map(|entry| entry.unwrap().path())
                .collect::<Vec<_>>();
            paths.sort();
            for path in paths {
                let relative = path.strip_prefix(root).unwrap().to_owned();
                if path.is_dir() {
                    output.push((relative, None));
                    collect(root, &path, output);
                } else {
                    let bytes = fs::read(path).unwrap();
                    output.push((relative, Some((bytes.len() as u64, sha256_hex(&bytes)))));
                }
            }
        }

        let mut output = Vec::new();
        collect(root, root, &mut output);
        output
    }

    fn persistent_tree_snapshot(root: &Path) -> Vec<(PathBuf, Option<(u64, String)>)> {
        snapshot_tree(root)
            .into_iter()
            .map(|(path, value)| {
                if path.file_name().and_then(|name| name.to_str()) == Some("queries.sqlite3-shm") {
                    (path, value.map(|(len, _)| (len, String::new())))
                } else {
                    (path, value)
                }
            })
            .collect()
    }

    #[test]
    fn statistics_report_an_absent_cache_without_creating_it() {
        let manifest = manifest("statistics-absent");
        let project_root = manifest.parent().unwrap();
        let before = snapshot_tree(project_root);

        let statistics = QueryStore::statistics(&manifest).unwrap();

        assert_eq!(
            statistics,
            QueryStoreStatistics::empty(
                project_root.join("generated/.blobray-cache"),
                project_root.join("generated/.blobray-cache/queries.sqlite3")
            )
        );
        assert_eq!(snapshot_tree(project_root), before);
        assert!(!project_root.join("generated").exists());
    }

    #[cfg(unix)]
    #[test]
    fn plan_guard_rejects_a_cold_root_with_a_symlinked_sqlite_sidecar() {
        use std::os::unix::fs::symlink;

        let manifest = manifest("plan-cold-symlink-sidecar");
        let project_root = manifest.parent().unwrap();
        let cache_root = project_root.join("generated/.blobray-cache");
        let external = project_root.join("external-wal");
        fs::create_dir_all(&cache_root).unwrap();
        fs::write(&external, b"caller-owned").unwrap();
        symlink(&external, cache_root.join("queries.sqlite3-wal")).unwrap();

        let error = QueryStore::plan_read_guard(&manifest).unwrap_err();

        assert!(error.to_string().contains("SQLite sidecar"));
        assert!(error.to_string().contains("not a regular file"));
        assert_eq!(fs::read(&external).unwrap(), b"caller-owned");
        assert!(!cache_root.join("queries.sqlite3").exists());
    }

    #[cfg(unix)]
    #[test]
    fn plan_guard_rejects_hostile_entries_in_a_cold_cache_root() {
        use std::{os::unix::fs::symlink, os::unix::net::UnixListener};

        let pack_manifest = manifest("plan-cold-symlink-pack");
        let pack_project = pack_manifest.parent().unwrap();
        let pack_root = pack_project.join("generated/.blobray-cache");
        let external_pack = pack_project.join("external.pack");
        fs::create_dir_all(&pack_root).unwrap();
        fs::write(&external_pack, b"caller-owned").unwrap();
        symlink(&external_pack, pack_root.join("objects-0.pack")).unwrap();

        let pack_error = QueryStore::plan_read_guard(&pack_manifest).unwrap_err();
        assert!(pack_error.to_string().contains("symbolic link"));
        assert_eq!(fs::read(&external_pack).unwrap(), b"caller-owned");
        assert!(!pack_root.join("queries.sqlite3").exists());

        let socket_manifest = manifest("plan-cold-unsupported-entry");
        let socket_root = socket_manifest
            .parent()
            .unwrap()
            .join("generated/.blobray-cache");
        fs::create_dir_all(&socket_root).unwrap();
        let socket = socket_root.join("hostile.sock");
        let _listener = UnixListener::bind(&socket).unwrap();

        let socket_error = QueryStore::plan_read_guard(&socket_manifest).unwrap_err();
        assert!(
            socket_error
                .to_string()
                .contains("unsupported filesystem entry")
        );
        assert!(!socket_root.join("queries.sqlite3").exists());
    }

    #[cfg(unix)]
    #[test]
    fn plan_guard_rejects_invalid_generated_parent_before_reporting_absent() {
        use std::os::unix::fs::symlink;

        let symlink_manifest = manifest("plan-absent-generated-symlink");
        let symlink_project = symlink_manifest.parent().unwrap();
        let external = symlink_project.join("external-generated");
        fs::create_dir(&external).unwrap();
        symlink(&external, symlink_project.join("generated")).unwrap();

        let symlink_error = QueryStore::plan_read_guard(&symlink_manifest).unwrap_err();
        assert!(symlink_error.to_string().contains("query cache parent"));
        assert!(
            symlink_error
                .to_string()
                .contains("not a regular directory")
        );
        assert!(fs::read_dir(&external).unwrap().next().is_none());

        let file_manifest = manifest("plan-absent-generated-file");
        let file_project = file_manifest.parent().unwrap();
        let generated = file_project.join("generated");
        fs::write(&generated, b"caller-owned").unwrap();

        let file_error = QueryStore::plan_read_guard(&file_manifest).unwrap_err();
        assert!(file_error.to_string().contains("query cache parent"));
        assert!(file_error.to_string().contains("not a regular directory"));
        assert_eq!(fs::read(&generated).unwrap(), b"caller-owned");
    }

    #[test]
    fn cache_lifetime_lock_serializes_writers_and_plan_snapshots() {
        let manifest = manifest("lifetime-lock");
        let writer = QueryStore::open(&manifest).unwrap();

        let second_writer = QueryStore::open(&manifest)
            .err()
            .expect("second writer must be rejected");
        assert!(
            second_writer
                .to_string()
                .contains("active writer or reader")
        );
        let reader = QueryStore::plan_read_guard(&manifest).unwrap_err();
        assert!(reader.to_string().contains("active writer or reader"));
        drop(writer);

        let snapshot = QueryStore::plan_read_guard(&manifest)
            .unwrap()
            .expect("initialized cache snapshot");
        let blocked_writer = QueryStore::open(&manifest)
            .err()
            .expect("writer must wait for the plan snapshot");
        assert!(
            blocked_writer
                .to_string()
                .contains("active writer or reader")
        );
        drop(snapshot);
        let reopened = QueryStore::open(&manifest).unwrap();
        drop(reopened);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn plan_guard_fails_closed_after_cache_root_generation_replacement() {
        let manifest = manifest("plan-pinned-generation");
        let project_root = manifest.parent().unwrap();
        let cache_root = project_root.join("generated/.blobray-cache");
        let retired_root = project_root.join("generated/.blobray-cache-generation-a");
        let generation_a_digest = {
            let mut writer = QueryStore::open(&manifest).unwrap();
            let cached = output(&manifest, "generation-a/output.json", b"generation a");
            let digest = cached.1.clone();
            writer
                .record_stage("symbol-inventory", "stage-signature", &[cached])
                .unwrap();
            digest
        };
        let guard = QueryStore::plan_read_guard(&manifest)
            .unwrap()
            .expect("generation A has a database");
        assert_eq!(guard.root, cache_root);
        assert!(guard.pinned_root.storage_root.starts_with("/proc/self/fd"));
        assert_eq!(
            QueryStore::stage_output_digests_read_only(&guard, "stage-signature", false,).unwrap(),
            Some(vec![generation_a_digest])
        );

        fs::rename(&cache_root, &retired_root).unwrap();
        {
            let mut writer = QueryStore::open(&manifest).unwrap();
            let cached = output(&manifest, "generation-b/output.json", b"generation b");
            writer
                .record_stage("symbol-inventory", "stage-signature", &[cached])
                .unwrap();
        }
        let generation_a_before = snapshot_tree(&retired_root);
        let generation_b_before = snapshot_tree(&cache_root);

        let error = QueryStore::stage_output_digests_read_only(&guard, "stage-signature", true)
            .unwrap_err();

        assert!(error.to_string().contains("was replaced"));
        assert_eq!(snapshot_tree(&retired_root), generation_a_before);
        assert_eq!(snapshot_tree(&cache_root), generation_b_before);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn replaced_writer_never_mutates_or_cleans_the_new_generation() {
        let manifest = manifest("writer-pinned-generation");
        let project_root = manifest.parent().unwrap();
        let cache_root = project_root.join("generated/.blobray-cache");
        let retired_root = project_root.join("generated/.blobray-cache-generation-a");
        let mut writer = QueryStore::open(&manifest).unwrap();
        writer
            .put(
                "generation-a",
                "function",
                "inputs",
                &[],
                &vec![0x31; INLINE_VALUE_LIMIT + 1],
            )
            .unwrap();
        assert_eq!(writer.root, cache_root);
        assert_eq!(writer.pack_path.parent(), Some(cache_root.as_path()));

        fs::rename(&cache_root, &retired_root).unwrap();
        {
            let mut generation_b = QueryStore::open(&manifest).unwrap();
            generation_b
                .put(
                    "generation-b",
                    "function",
                    "inputs",
                    &[],
                    &vec![0x42; INLINE_VALUE_LIMIT + 1],
                )
                .unwrap();
        }
        fs::write(
            cache_root.join("objects-999.pack"),
            b"generation-b sentinel",
        )
        .unwrap();
        let generation_b_before = snapshot_tree(&cache_root);

        let put_error = writer
            .put("late", "function", "inputs", &[], b"late")
            .unwrap_err();
        assert!(put_error.to_string().contains("was replaced"));
        assert!(
            writer
                .compact()
                .unwrap_err()
                .to_string()
                .contains("was replaced")
        );
        assert!(
            writer
                .remove_unreferenced_pack_files()
                .unwrap_err()
                .to_string()
                .contains("was replaced")
        );
        assert_eq!(snapshot_tree(&cache_root), generation_b_before);

        drop(writer);

        assert_eq!(snapshot_tree(&cache_root), generation_b_before);
        assert_eq!(
            fs::read(cache_root.join("objects-999.pack")).unwrap(),
            b"generation-b sentinel"
        );
    }

    #[test]
    fn statistics_measure_queries_objects_and_reclaimable_records_read_only() {
        let manifest = manifest("statistics-populated");
        let project_root = manifest.parent().unwrap();
        let small = b"small";
        let large = vec![0x5a; INLINE_VALUE_LIMIT + 1];
        let live_output = b"live-stage-output";
        let retired_output = b"retired-stage-output";
        let live_digest = sha256_hex(live_output);
        {
            let mut store = QueryStore::open(&manifest).unwrap();
            store
                .put(
                    "small",
                    "function",
                    "small-inputs",
                    &["dependency-a".to_owned(), "dependency-b".to_owned()],
                    small,
                )
                .unwrap();
            store
                .put("large-a", "function", "large-a-inputs", &[], &large)
                .unwrap();
            store
                .put("large-b", "function", "large-b-inputs", &[], &large)
                .unwrap();
            let live = output(&manifest, "live/output.json", live_output);
            store
                .record_stage("live-stage", "live-query", &[live])
                .unwrap();
            let retired = output(&manifest, "retired/output.json", retired_output);
            store
                .record_stage("retired-stage", "retired-query", &[retired])
                .unwrap();
            store
                .record_stage("retired-stage", "replacement-query", &[])
                .unwrap();
        }
        let database = project_root.join("generated/.blobray-cache/queries.sqlite3");
        assert!(!PathBuf::from(format!("{}-wal", database.display())).exists());
        assert!(!PathBuf::from(format!("{}-shm", database.display())).exists());
        fs::write(sqlite_sidecar_path(&database, "-wal"), []).unwrap();
        fs::write(
            sqlite_sidecar_path(&database, "-shm"),
            vec![0_u8; 32 * 1024],
        )
        .unwrap();
        let before = snapshot_tree(project_root);

        let statistics = QueryStore::statistics(&manifest).unwrap();

        let live_query_bytes = serde_json::to_vec(&vec![live_digest]).unwrap().len() as u64;
        assert!(statistics.present);
        assert_eq!(statistics.schema, Some(STORE_SCHEMA as u32));
        assert_eq!(statistics.query_results, 5);
        assert_eq!(
            statistics.query_kinds,
            vec![
                QueryKindStatistics {
                    kind: "function".to_owned(),
                    query_results: 3,
                    inline_bytes: small.len() as u64,
                },
                QueryKindStatistics {
                    kind: "project-stage".to_owned(),
                    query_results: 2,
                    inline_bytes: live_query_bytes + 2,
                },
            ]
        );
        assert_eq!(
            statistics.inline_bytes,
            small.len() as u64 + live_query_bytes + 2
        );
        assert_eq!(statistics.dependencies, 2);
        assert_eq!(statistics.objects, 3);
        assert_eq!(
            statistics.object_payload_bytes,
            (large.len() + live_output.len() + retired_output.len()) as u64
        );
        assert_eq!(statistics.stage_bindings, 2);
        assert_eq!(statistics.stage_outputs, 1);
        assert_eq!(statistics.live_objects, 2);
        assert_eq!(
            statistics.live_record_bytes,
            PACK_HEADER_BYTES * 2 + large.len() as u64 + live_output.len() as u64
        );
        assert_eq!(
            statistics.pack_bytes,
            PACK_HEADER_BYTES * 3
                + large.len() as u64
                + live_output.len() as u64
                + retired_output.len() as u64
        );
        assert_eq!(
            statistics.reclaimable_pack_bytes,
            PACK_HEADER_BYTES + retired_output.len() as u64
        );
        assert!(statistics.database_bytes > 0);
        assert!(statistics.root_bytes >= statistics.database_bytes + statistics.pack_bytes);
        assert_eq!(snapshot_tree(project_root), before);
        assert_eq!(
            fs::metadata(sqlite_sidecar_path(&database, "-wal"))
                .unwrap()
                .len(),
            0
        );
        assert_eq!(
            fs::metadata(sqlite_sidecar_path(&database, "-shm"))
                .unwrap()
                .len(),
            32 * 1024
        );
    }

    #[test]
    fn statistics_reject_an_unsupported_schema_without_resetting_it() {
        let manifest = manifest("statistics-unsupported");
        let project_root = manifest.parent().unwrap();
        {
            let store = QueryStore::open(&manifest).unwrap();
            store
                .connection
                .execute_batch("PRAGMA user_version=99;")
                .unwrap();
        }
        let before = snapshot_tree(project_root);

        let error = QueryStore::statistics(&manifest).unwrap_err();

        assert!(error.to_string().contains("schema 99 is unsupported"));
        assert_eq!(snapshot_tree(project_root), before);
    }

    #[test]
    fn statistics_fail_closed_while_a_wal_writer_is_active() {
        let manifest = manifest("statistics-active-writer");
        let project_root = manifest.parent().unwrap();
        let mut store = QueryStore::open(&manifest).unwrap();
        store
            .put("live", "function", "inputs", &[], b"live result")
            .unwrap();
        let database = project_root.join("generated/.blobray-cache/queries.sqlite3");
        assert!(sqlite_sidecar_path(&database, "-wal").exists());
        assert!(
            fs::metadata(sqlite_sidecar_path(&database, "-wal"))
                .unwrap()
                .len()
                > 0
        );
        assert!(sqlite_sidecar_path(&database, "-shm").exists());
        let before = snapshot_tree(project_root);

        let error = QueryStore::statistics(&manifest).unwrap_err();

        assert!(error.to_string().contains("active SQLite WAL"));
        assert!(
            error
                .to_string()
                .contains("retry after the cache writer exits")
        );
        assert_eq!(snapshot_tree(project_root), before);
    }

    #[test]
    fn statistics_fail_closed_when_a_writer_starts_after_preflight() {
        let manifest = manifest("statistics-postflight-writer");
        {
            let _store = QueryStore::open(&manifest).unwrap();
        }

        let error = QueryStore::statistics_after_preflight(&manifest, || {
            let mut writer = QueryStore::open(&manifest).unwrap();
            writer
                .put("late", "function", "inputs", &[], b"late result")
                .unwrap();
            writer
        })
        .unwrap_err();

        assert!(error.to_string().contains("active SQLite WAL"));
        assert!(
            error
                .to_string()
                .contains("retry after the cache writer exits")
        );
    }

    #[test]
    fn statistics_fail_closed_when_cache_changes_after_preflight() {
        let manifest = manifest("statistics-postflight-change");
        {
            let _store = QueryStore::open(&manifest).unwrap();
        }
        let database = manifest
            .parent()
            .unwrap()
            .join("generated/.blobray-cache/queries.sqlite3");

        let error = QueryStore::statistics_after_preflight(&manifest, || {
            let mut writer = QueryStore::open(&manifest).unwrap();
            writer
                .put("late", "function", "inputs", &[], b"late result")
                .unwrap();
            drop(writer);
            assert!(!sqlite_sidecar_path(&database, "-wal").exists());
        })
        .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("changed during read-only statistics inspection")
        );
        assert!(
            error
                .to_string()
                .contains("retry after the cache writer exits")
        );
    }

    #[cfg(unix)]
    #[test]
    fn statistics_reject_symlinked_databases_and_packs() {
        use std::os::unix::fs::symlink;

        let database_manifest = manifest("statistics-database-symlink");
        {
            let _store = QueryStore::open(&database_manifest).unwrap();
        }
        let database_root = database_manifest.parent().unwrap();
        let database = database_root.join("generated/.blobray-cache/queries.sqlite3");
        let real_database = database_root.join("real-queries.sqlite3");
        fs::rename(&database, &real_database).unwrap();
        symlink(&real_database, &database).unwrap();

        let error = QueryStore::statistics(&database_manifest).unwrap_err();
        assert!(error.to_string().contains("is not a regular file"));

        let pack_manifest = manifest("statistics-pack-symlink");
        let pack = {
            let mut store = QueryStore::open(&pack_manifest).unwrap();
            store
                .put(
                    "large",
                    "function",
                    "inputs",
                    &[],
                    &vec![0x5a; INLINE_VALUE_LIMIT + 1],
                )
                .unwrap();
            store.pack_path.clone()
        };
        let pack_root = pack_manifest.parent().unwrap();
        let real_pack = pack_root.join("real-objects.pack");
        fs::rename(&pack, &real_pack).unwrap();
        symlink(&real_pack, &pack).unwrap();

        let error = QueryStore::statistics(&pack_manifest).unwrap_err();
        assert!(error.to_string().contains("contains symbolic link"));
    }

    #[test]
    fn statistics_reject_a_corrupt_database_without_writing_sidecars() {
        let manifest = manifest("statistics-corrupt");
        let project_root = manifest.parent().unwrap();
        let cache_root = project_root.join("generated/.blobray-cache");
        fs::create_dir_all(&cache_root).unwrap();
        fs::write(cache_root.join("queries.sqlite3"), b"not a sqlite database").unwrap();
        let before = snapshot_tree(project_root);

        let error = QueryStore::statistics(&manifest).unwrap_err();

        assert!(error.to_string().contains("query database schema"));
        assert_eq!(snapshot_tree(project_root), before);
        assert!(!cache_root.join("queries.sqlite3-wal").exists());
        assert!(!cache_root.join("queries.sqlite3-shm").exists());
    }

    #[test]
    fn stores_small_values_inline_and_large_values_once_in_the_pack() {
        let manifest = manifest("values");
        let mut store = QueryStore::open(&manifest).unwrap();
        let small = b"function-summary";
        let large = vec![0x5a; INLINE_VALUE_LIMIT + 1];
        store
            .put("small", "function", "inputs-a", &[], small)
            .unwrap();
        store
            .put("large-a", "function", "inputs-b", &[], &large)
            .unwrap();
        let first_pack_length = fs::metadata(&store.pack_path).unwrap().len();
        store
            .put("large-b", "function", "inputs-c", &[], &large)
            .unwrap();

        assert_eq!(store.get("small").unwrap().unwrap(), small);
        assert_eq!(store.get("large-a").unwrap().unwrap(), large);
        assert_eq!(store.get("large-b").unwrap().unwrap(), large);
        assert_eq!(
            fs::metadata(&store.pack_path).unwrap().len(),
            first_pack_length
        );
    }

    #[test]
    fn pack_replacement_after_fsync_is_rejected_before_sqlite_indexing() {
        let manifest = manifest("pack-replaced-before-index");
        let mut store = QueryStore::open(&manifest).unwrap();
        let value = vec![0x5a; INLINE_VALUE_LIMIT + 1];
        let digest = sha256_hex(&value);
        let active_pack = store.pack_path.clone();
        let detached_pack = active_pack.with_file_name("detached-objects.pack");

        let error = store
            .ensure_objects_after_pack_sync(
                std::iter::once((digest.as_str(), value.as_slice())),
                || {
                    fs::rename(&active_pack, &detached_pack).unwrap();
                    fs::write(&active_pack, b"replacement pack").unwrap();
                },
            )
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("changed while it was being opened")
        );
        assert_eq!(fs::read(&active_pack).unwrap(), b"replacement pack");
        assert!(fs::metadata(&detached_pack).unwrap().len() > PACK_HEADER_BYTES);
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM objects WHERE digest = ?1",
                    [&digest],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );
    }

    #[test]
    fn writer_revalidates_the_pinned_database_binding_before_operations_and_drop() {
        let manifest = manifest("database-replaced-after-open");
        let project_root = manifest.parent().unwrap();
        let database = project_root.join("generated/.blobray-cache/queries.sqlite3");
        let detached_database = project_root.join("detached-queries.sqlite3");
        let mut store = QueryStore::open(&manifest).unwrap();
        fs::rename(&database, &detached_database).unwrap();
        fs::write(&database, b"replacement database").unwrap();

        let error = store
            .put("late", "function", "inputs", &[], b"late value")
            .unwrap_err();

        assert!(
            error
                .to_string()
                .contains("changed while it was being opened")
        );
        assert_eq!(fs::read(&database).unwrap(), b"replacement database");
        drop(store);
        assert_eq!(fs::read(&database).unwrap(), b"replacement database");
    }

    #[test]
    fn stage_plan_treats_an_obsolete_schema_as_a_cold_cache_without_resetting_it() {
        let manifest = manifest("stage-plan-obsolete-schema");
        let project_root = manifest.parent().unwrap();
        {
            let mut store = QueryStore::open(&manifest).unwrap();
            let cached = output(&manifest, "generated/output.json", b"cached output");
            store
                .record_stage("symbol-inventory", "stage-signature", &[cached])
                .unwrap();
            store
                .connection
                .execute_batch("PRAGMA user_version=99;")
                .unwrap();
        }
        let before = snapshot_tree(project_root);

        let result = read_only_stage_digests(&manifest, "stage-signature", false).unwrap();

        assert_eq!(result, None);
        assert_eq!(snapshot_tree(project_root), before);
    }

    #[test]
    fn stage_plan_hashes_the_complete_cached_output_payload_read_only() {
        let manifest = manifest("stage-plan-corrupt-payload");
        let project_root = manifest.parent().unwrap();
        let (pack_path, payload_offset) = {
            let mut store = QueryStore::open(&manifest).unwrap();
            let cached = output(&manifest, "generated/output.json", b"cached output");
            let digest = cached.1.clone();
            store
                .record_stage("symbol-inventory", "stage-signature", &[cached])
                .unwrap();
            let offset = store
                .connection
                .query_row(
                    "SELECT pack_offset FROM objects WHERE digest = ?1",
                    [&digest],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap();
            (
                store.pack_path.clone(),
                u64::try_from(offset).unwrap() + PACK_HEADER_BYTES,
            )
        };
        let mut pack = OpenOptions::new()
            .read(true)
            .write(true)
            .open(pack_path)
            .unwrap();
        pack.seek(SeekFrom::Start(payload_offset)).unwrap();
        let mut byte = [0_u8; 1];
        pack.read_exact(&mut byte).unwrap();
        byte[0] ^= 0xff;
        pack.seek(SeekFrom::Start(payload_offset)).unwrap();
        pack.write_all(&byte).unwrap();
        pack.sync_all().unwrap();
        drop(pack);
        let before = snapshot_tree(project_root);

        let error = read_only_stage_digests(&manifest, "stage-signature", true).unwrap_err();

        assert!(error.to_string().contains("failed its content digest"));
        assert_eq!(snapshot_tree(project_root), before);
    }

    #[test]
    fn cached_object_pack_names_cannot_escape_the_cache_root() {
        let manifest = manifest("object-pack-path");
        let project_root = manifest.parent().unwrap();
        {
            let mut store = QueryStore::open(&manifest).unwrap();
            let cached = output(&manifest, "generated/output.json", b"cached output");
            store
                .record_stage("symbol-inventory", "stage-signature", &[cached])
                .unwrap();
            store
                .connection
                .execute("UPDATE objects SET pack_name = '../outside.pack'", [])
                .unwrap();
        }
        let before = snapshot_tree(project_root);

        let error = read_only_stage_digests(&manifest, "stage-signature", true).unwrap_err();

        assert!(error.to_string().contains("invalid pack name"));
        assert_eq!(snapshot_tree(project_root), before);
    }

    #[test]
    fn writer_rejects_an_active_pack_name_outside_the_cache_root() {
        let manifest = manifest("active-pack-path");
        let project_root = manifest.parent().unwrap();
        let outside = project_root.join("outside.pack");
        fs::write(&outside, b"caller-owned").unwrap();
        {
            let store = QueryStore::open(&manifest).unwrap();
            store
                .connection
                .execute(
                    "UPDATE cache_state SET active_pack = '../outside.pack' WHERE singleton = 1",
                    [],
                )
                .unwrap();
        }

        let error = QueryStore::open(&manifest)
            .err()
            .expect("invalid active pack must fail writer open");

        assert!(error.to_string().contains("invalid active pack name"));
        assert_eq!(fs::read(&outside).unwrap(), b"caller-owned");
    }

    #[cfg(unix)]
    #[test]
    fn writer_rejects_symlinked_cache_roots_databases_and_active_packs() {
        use std::os::unix::fs::symlink;

        let root_manifest = manifest("writer-root-symlink");
        let root_project = root_manifest.parent().unwrap();
        let generated = root_project.join("generated");
        let external_root = root_project.join("external-cache");
        fs::create_dir_all(&generated).unwrap();
        fs::create_dir_all(&external_root).unwrap();
        symlink(&external_root, generated.join(".blobray-cache")).unwrap();
        let error = QueryStore::open(&root_manifest)
            .err()
            .expect("symlinked cache root must fail writer open");
        assert!(error.to_string().contains("not a regular directory"));
        assert!(fs::read_dir(&external_root).unwrap().next().is_none());

        let database_manifest = manifest("writer-database-symlink");
        {
            let _store = QueryStore::open(&database_manifest).unwrap();
        }
        let database_project = database_manifest.parent().unwrap();
        let database = database_project.join("generated/.blobray-cache/queries.sqlite3");
        let external_database = database_project.join("external.sqlite3");
        fs::rename(&database, &external_database).unwrap();
        symlink(&external_database, &database).unwrap();
        let error = QueryStore::open(&database_manifest)
            .err()
            .expect("symlinked database must fail writer open");
        assert!(error.to_string().contains("not a regular file"));

        let sidecar_manifest = manifest("writer-sidecar-symlink");
        {
            let _store = QueryStore::open(&sidecar_manifest).unwrap();
        }
        let sidecar_project = sidecar_manifest.parent().unwrap();
        let sidecar_database = sidecar_project.join("generated/.blobray-cache/queries.sqlite3");
        let external_sidecar = sidecar_project.join("external-wal");
        fs::write(&external_sidecar, b"caller-owned").unwrap();
        symlink(
            &external_sidecar,
            sqlite_sidecar_path(&sidecar_database, "-wal"),
        )
        .unwrap();
        let error = QueryStore::open(&sidecar_manifest)
            .err()
            .expect("symlinked SQLite sidecar must fail writer open");
        assert!(error.to_string().contains("SQLite sidecar"));
        assert_eq!(fs::read(&external_sidecar).unwrap(), b"caller-owned");

        let pack_manifest = manifest("writer-pack-symlink");
        let mut store = QueryStore::open(&pack_manifest).unwrap();
        let external_pack = pack_manifest.parent().unwrap().join("external.pack");
        fs::write(&external_pack, b"caller-owned").unwrap();
        symlink(&external_pack, &store.pack_path).unwrap();
        let error = store
            .put(
                "large",
                "function",
                "inputs",
                &[],
                &vec![0x5a; INLINE_VALUE_LIMIT + 1],
            )
            .unwrap_err();
        assert!(error.to_string().contains("not a regular file"));
        assert_eq!(fs::read(&external_pack).unwrap(), b"caller-owned");
    }

    #[test]
    fn stage_binding_is_replaced_without_retaining_an_old_owner() {
        let manifest = manifest("stage");
        let mut store = QueryStore::open(&manifest).unwrap();
        let first = output(&manifest, "ap/functions.jsonl", b"function-a");
        store
            .record_stage("linked-ir:ap", "query-a", &[first])
            .unwrap();
        assert!(store.stage_output_digests("query-a").unwrap().is_some());
        store.record_stage("linked-ir:ap", "query-b", &[]).unwrap();
        assert!(store.stage_output_digests("query-a").unwrap().is_none());
        assert_eq!(
            store.stage_output_digests("query-b").unwrap(),
            Some(Vec::new())
        );
    }

    #[test]
    fn equivalent_stage_bindings_share_one_query_result() {
        let manifest = manifest("shared-stage");
        let mut store = QueryStore::open(&manifest).unwrap();
        let focused_outputs = vec![output(
            &manifest,
            "focused/functions.jsonl",
            b"same-function",
        )];
        let full_outputs = vec![output(&manifest, "full/functions.jsonl", b"same-function")];
        store
            .record_stage("linked-ir:focused", "shared-query", &focused_outputs)
            .unwrap();
        store
            .record_stage("linked-ir:full", "shared-query", &full_outputs)
            .unwrap();
        store
            .record_stage("linked-ir:focused", "new-query", &[])
            .unwrap();

        assert_eq!(
            store.stage_output_digests("shared-query").unwrap(),
            Some(vec![sha256_hex(b"same-function")])
        );
    }

    #[test]
    fn stage_binding_retirement_is_conditional_and_preserves_shared_owners() {
        let manifest = manifest("retire-shared-stage");
        let mut store = QueryStore::open(&manifest).unwrap();
        let focused = output(&manifest, "focused/functions.jsonl", b"same-function");
        let full = output(&manifest, "full/functions.jsonl", b"same-function");
        store
            .record_stage("linked-ir:focused", "shared-query", &[focused])
            .unwrap();
        store
            .record_stage("linked-ir:full", "shared-query", &[full])
            .unwrap();

        store
            .retire_stage_binding("linked-ir:focused", "shared-query")
            .unwrap();
        assert_eq!(
            store.stage_output_digests("shared-query").unwrap(),
            Some(vec![sha256_hex(b"same-function")])
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM stage_outputs WHERE stage = ?1",
                    ["linked-ir:focused"],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            0
        );

        store
            .retire_stage_binding("linked-ir:full", "different-query")
            .unwrap();
        assert!(
            store
                .stage_output_digests("shared-query")
                .unwrap()
                .is_some()
        );
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM stage_outputs WHERE stage = ?1",
                    ["linked-ir:full"],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap(),
            1
        );

        store
            .retire_stage_binding("linked-ir:full", "shared-query")
            .unwrap();
        assert!(
            store
                .stage_output_digests("shared-query")
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM stage_bindings", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM stage_outputs", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            0
        );
    }

    #[test]
    fn cached_output_is_restored_atomically_at_a_new_path() {
        let manifest = manifest("restore");
        let mut store = QueryStore::open(&manifest).unwrap();
        let source = output(&manifest, "focused/functions.jsonl", b"function-facts");
        let digest = source.1.clone();
        store
            .record_stage("linked-ir:focused", "shared-query", &[source])
            .unwrap();
        let destination = manifest.parent().unwrap().join("full/functions.jsonl");

        store.restore_output(&digest, &destination).unwrap();

        assert_eq!(fs::read(destination).unwrap(), b"function-facts");
    }

    #[test]
    fn a_query_key_cannot_be_rebound_to_different_bytes() {
        let manifest = manifest("immutable-query");
        let mut store = QueryStore::open(&manifest).unwrap();
        store
            .put("function-key", "function", "inputs", &[], b"result-a")
            .unwrap();
        let error = store
            .put("function-key", "function", "inputs", &[], b"result-b")
            .unwrap_err();
        assert!(error.to_string().contains("different immutable result"));
        assert_eq!(store.get("function-key").unwrap().unwrap(), b"result-a");
    }

    #[test]
    fn function_facts_are_published_as_one_immutable_batch() {
        let manifest = manifest("function-fact-batch");
        let mut store = QueryStore::open(&manifest).unwrap();
        let facts = vec![
            ("function-direct:a".to_owned(), b"fact-a".to_vec()),
            ("function-direct:b".to_owned(), b"fact-b".to_vec()),
            (
                "function-direct:large".to_owned(),
                vec![0x5a; INLINE_VALUE_LIMIT + 1],
            ),
        ];

        store.put_function_fact_batch(&facts).unwrap();
        store.put_function_fact_batch(&facts).unwrap();

        assert_eq!(store.get("function-direct:a").unwrap().unwrap(), b"fact-a");
        assert_eq!(store.get("function-direct:b").unwrap().unwrap(), b"fact-b");
        assert_eq!(
            <QueryStore as crate::analysis::FunctionFactStore>::load_function_facts(
                &store,
                &["function-direct:large".to_owned()]
            )
            .unwrap(),
            vec![(
                "function-direct:large".to_owned(),
                vec![0x5a; INLINE_VALUE_LIMIT + 1]
            )]
        );
    }

    #[test]
    fn function_fact_preflight_batches_mixed_hits_misses_and_cross_boundary_duplicates() {
        let manifest = manifest("function-fact-batched-preflight");
        let mut store = QueryStore::open(&manifest).unwrap();
        let facts = (0..FUNCTION_FACT_LOOKUP_BATCH * 2 + 3)
            .map(|index| {
                (
                    format!("function-direct:{index:04}"),
                    format!("fact-{index:04}").into_bytes(),
                )
            })
            .collect::<Vec<_>>();
        let existing = facts.iter().step_by(64).cloned().collect::<Vec<_>>();
        store.put_function_fact_batch(&existing).unwrap();
        let mut requested = facts.clone();
        requested.push(facts[0].clone());
        let mut statement_sizes = Vec::new();

        store
            .put_function_fact_batch_observed(&requested, |size| statement_sizes.push(size))
            .unwrap();

        assert_eq!(
            statement_sizes,
            vec![FUNCTION_FACT_LOOKUP_BATCH, FUNCTION_FACT_LOOKUP_BATCH, 3,]
        );
        assert!(
            statement_sizes
                .iter()
                .all(|size| *size <= FUNCTION_FACT_LOOKUP_BATCH)
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM query_results", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            facts.len() as i64
        );
        assert_eq!(store.get(&facts[0].0).unwrap().unwrap(), facts[0].1);
        assert_eq!(
            store.get(&facts.last().unwrap().0).unwrap().unwrap(),
            facts.last().unwrap().1
        );
    }

    #[test]
    fn function_fact_preflight_mismatch_in_final_batch_is_atomic() {
        for mismatch in ["kind", "input", "digest"] {
            let manifest = manifest(&format!("function-fact-preflight-{mismatch}"));
            let mut store = QueryStore::open(&manifest).unwrap();
            let facts = (0..FUNCTION_FACT_LOOKUP_BATCH * 2 + 3)
                .map(|index| {
                    let value = if index == 0 {
                        vec![0x5a; INLINE_VALUE_LIMIT + 1]
                    } else {
                        format!("fact-{index:04}").into_bytes()
                    };
                    (format!("function-direct:{index:04}"), value)
                })
                .collect::<Vec<_>>();
            let (last_key, last_value) = facts.last().unwrap();
            match mismatch {
                "kind" => {
                    store
                        .put(last_key, "project-stage", last_key, &[], last_value)
                        .unwrap();
                }
                "input" => {
                    store
                        .put(
                            last_key,
                            "function-direct-fact",
                            "wrong-input",
                            &[],
                            last_value,
                        )
                        .unwrap();
                }
                "digest" => {
                    store
                        .put(
                            last_key,
                            "function-direct-fact",
                            last_key,
                            &[],
                            b"different-value",
                        )
                        .unwrap();
                }
                _ => unreachable!(),
            }
            let project_root = manifest.parent().unwrap();
            let before = persistent_tree_snapshot(project_root);
            let mut statement_sizes = Vec::new();

            let error = store
                .put_function_fact_batch_observed(&facts, |size| statement_sizes.push(size))
                .unwrap_err();

            assert!(error.to_string().contains("different immutable result"));
            assert_eq!(
                statement_sizes,
                vec![FUNCTION_FACT_LOOKUP_BATCH, FUNCTION_FACT_LOOKUP_BATCH, 3,]
            );
            assert_eq!(persistent_tree_snapshot(project_root), before);
            assert!(!store.pack_path.exists());
            assert_eq!(
                store
                    .connection
                    .query_row("SELECT COUNT(*) FROM query_results", [], |row| {
                        row.get::<_, i64>(0)
                    })
                    .unwrap(),
                1
            );
        }
    }

    #[test]
    fn function_fact_preflight_empty_idempotent_and_duplicate_inputs_are_explicit() {
        let manifest = manifest("function-fact-preflight-idempotent");
        let project_root = manifest.parent().unwrap();
        let mut store = QueryStore::open(&manifest).unwrap();
        let before_empty = snapshot_tree(project_root);
        let mut empty_statements = Vec::new();
        store
            .put_function_fact_batch_observed(&[], |size| empty_statements.push(size))
            .unwrap();
        assert!(empty_statements.is_empty());
        assert_eq!(snapshot_tree(project_root), before_empty);

        let identical = vec![
            ("function-direct:same".to_owned(), b"same-value".to_vec()),
            ("function-direct:same".to_owned(), b"same-value".to_vec()),
        ];
        let mut first_statements = Vec::new();
        store
            .put_function_fact_batch_observed(&identical, |size| first_statements.push(size))
            .unwrap();
        assert_eq!(first_statements, vec![1]);

        let before_idempotent = persistent_tree_snapshot(project_root);
        let mut idempotent_statements = Vec::new();
        store
            .put_function_fact_batch_observed(&identical, |size| idempotent_statements.push(size))
            .unwrap();
        assert_eq!(idempotent_statements, vec![1]);
        assert_eq!(persistent_tree_snapshot(project_root), before_idempotent);

        let conflicting = vec![
            (
                "function-direct:z-conflict".to_owned(),
                vec![0x11; INLINE_VALUE_LIMIT + 1],
            ),
            (
                "function-direct:z-conflict".to_owned(),
                vec![0x12; INLINE_VALUE_LIMIT + 1],
            ),
            (
                "function-direct:a-conflict".to_owned(),
                vec![0x21; INLINE_VALUE_LIMIT + 1],
            ),
            (
                "function-direct:a-conflict".to_owned(),
                vec![0x22; INLINE_VALUE_LIMIT + 1],
            ),
        ];
        let before_conflict = snapshot_tree(project_root);
        let mut conflicting_statements = Vec::new();
        let error = store
            .put_function_fact_batch_observed(&conflicting, |size| {
                conflicting_statements.push(size)
            })
            .unwrap_err();
        assert!(error.to_string().contains("different immutable result"));
        assert!(error.to_string().contains("function-direct:a-conflict"));
        assert!(conflicting_statements.is_empty());
        assert_eq!(snapshot_tree(project_root), before_conflict);
        assert!(!store.pack_path.exists());
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM objects", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .connection
                .query_row("SELECT COUNT(*) FROM query_results", [], |row| {
                    row.get::<_, i64>(0)
                })
                .unwrap(),
            1
        );
    }

    #[test]
    fn function_fact_loading_preserves_order_and_uses_bounded_statements() {
        let manifest = manifest("function-fact-batched-load");
        let mut store = QueryStore::open(&manifest).unwrap();
        let fact_count = FUNCTION_FACT_LOOKUP_BATCH * 2 + 3;
        let facts = (0..fact_count)
            .map(|index| {
                (
                    format!("function-direct:{index:04}"),
                    format!("fact-{index:04}").into_bytes(),
                )
            })
            .collect::<Vec<_>>();
        store.put_function_fact_batch(&facts).unwrap();
        store
            .put(
                "function-direct:wrong-kind",
                "project-stage",
                "inputs",
                &[],
                b"not-a-function-fact",
            )
            .unwrap();

        let mut keys = facts
            .iter()
            .rev()
            .map(|(key, _)| key.clone())
            .collect::<Vec<_>>();
        keys.insert(17, "function-direct:missing".to_owned());
        keys.push(facts[7].0.clone());
        keys.push("function-direct:wrong-kind".to_owned());
        let expected_by_key = facts
            .iter()
            .cloned()
            .collect::<std::collections::BTreeMap<_, _>>();
        let expected = keys
            .iter()
            .filter_map(|key| {
                expected_by_key
                    .get(key)
                    .map(|value| (key.clone(), value.clone()))
            })
            .collect::<Vec<_>>();
        let mut statement_sizes = Vec::new();

        let loaded = store
            .load_function_facts_batched(&keys, |size| statement_sizes.push(size))
            .unwrap();

        assert_eq!(loaded, expected);
        assert_eq!(
            statement_sizes.len(),
            keys.len().div_ceil(FUNCTION_FACT_LOOKUP_BATCH)
        );
        assert_eq!(
            statement_sizes,
            vec![
                FUNCTION_FACT_LOOKUP_BATCH,
                FUNCTION_FACT_LOOKUP_BATCH,
                keys.len() - FUNCTION_FACT_LOOKUP_BATCH * 2,
            ]
        );
        assert!(
            statement_sizes
                .iter()
                .all(|size| *size <= FUNCTION_FACT_LOOKUP_BATCH)
        );

        let mut empty_statement_sizes = Vec::new();
        assert!(
            store
                .load_function_facts_batched(&[], |size| empty_statement_sizes.push(size))
                .unwrap()
                .is_empty()
        );
        assert!(empty_statement_sizes.is_empty());
    }

    #[test]
    fn batched_function_fact_loading_rejects_digest_corruption() {
        let manifest = manifest("function-fact-batched-digest");
        let mut store = QueryStore::open(&manifest).unwrap();
        let key = "function-direct:corrupt".to_owned();
        store
            .put_function_fact_batch(&[(key.clone(), b"original-fact".to_vec())])
            .unwrap();
        store
            .connection
            .execute(
                "UPDATE query_results SET inline_value = ?2 WHERE query_key = ?1",
                params![&key, b"corrupt-fact"],
            )
            .unwrap();

        let error = <QueryStore as crate::analysis::FunctionFactStore>::load_function_facts(
            &store,
            std::slice::from_ref(&key),
        )
        .unwrap_err();

        assert!(error.to_string().contains(&format!("{key:?}")));
        assert!(error.to_string().contains("failed its content digest"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn compaction_keeps_only_reachable_objects_and_switches_pack_atomically() {
        let manifest = manifest("compact");
        let mut store = QueryStore::open(&manifest).unwrap();
        let live = vec![0x31; INLINE_VALUE_LIMIT + 1];
        store
            .put("live-function", "function", "inputs", &[], &live)
            .unwrap();
        let retired = output(&manifest, "old/functions.jsonl", b"retired-stage-output");
        let retired_digest = retired.1.clone();
        store
            .record_stage("linked-ir:old", "old-query", &[retired])
            .unwrap();
        store
            .record_stage("linked-ir:old", "new-query", &[])
            .unwrap();
        let old_pack = store.pack_path.clone();

        store.compact().unwrap();

        assert_ne!(store.pack_path, old_pack);
        assert!(!old_pack.exists());
        assert_eq!(store.get("live-function").unwrap().unwrap(), live);
        assert!(store.open_object(&retired_digest).is_err());
        assert_eq!(pack_files(&store.root).unwrap(), vec![store.pack_path]);
    }

    #[test]
    fn compaction_assessment_requires_size_bytes_and_ratio_thresholds() {
        let minimum = compaction_statistics(COMPACT_MIN_PACK_BYTES, 0);
        assert!(!minimum.eligible_on_next_write);

        let too_small =
            compaction_statistics(COMPACT_MIN_PACK_BYTES - 1, COMPACT_MIN_RECLAIMABLE_BYTES);
        assert!(!too_small.eligible_on_next_write);

        let too_little_garbage =
            compaction_statistics(COMPACT_MIN_PACK_BYTES, COMPACT_MIN_RECLAIMABLE_BYTES - 1);
        assert!(!too_little_garbage.eligible_on_next_write);

        let eligible = compaction_statistics(COMPACT_MIN_PACK_BYTES, COMPACT_MIN_RECLAIMABLE_BYTES);
        assert_eq!(eligible.supported, cfg!(target_os = "linux"));
        assert_eq!(eligible.automatic, cfg!(target_os = "linux"));
        assert_eq!(eligible.eligible_on_next_write, cfg!(target_os = "linux"));
    }
}
