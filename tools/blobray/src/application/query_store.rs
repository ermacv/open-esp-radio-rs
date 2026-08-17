//! Persistent cache owned by the demand-driven analysis engine.
//!
//! SQLite owns query identity, dependency edges and result locations. Large
//! immutable values live in one append-only content-addressed pack so the
//! cache does not create one filesystem object per function or force SQLite's
//! WAL to carry large analysis payloads.

use std::{
    collections::BTreeSet,
    fs::{self, File, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

use crate::Result;

const STORE_SCHEMA: i64 = 4;
const INLINE_VALUE_LIMIT: usize = 64 * 1024;
const PACK_RECORD_MAGIC: &[u8; 8] = b"VBWQCAS1";
const PACK_HEADER_BYTES: u64 = 8 + 32 + 8;
const COMPACT_MIN_PACK_BYTES: u64 = 256 * 1024 * 1024;
const COMPACT_MIN_RECLAIMABLE_BYTES: u64 = 64 * 1024 * 1024;
static NEXT_RESTORE_ID: AtomicU64 = AtomicU64::new(0);

pub(crate) struct QueryStore {
    connection: Connection,
    root: PathBuf,
    pack_path: PathBuf,
    next_pack_generation: u64,
}

impl QueryStore {
    pub(crate) fn open(project_manifest: &Path) -> Result<Self> {
        let project_root = project_manifest.parent().unwrap_or_else(|| Path::new("."));
        let root = project_root.join("generated/.blobray-cache");
        fs::create_dir_all(&root)?;
        let database_path = root.join("queries.sqlite3");
        let connection = Connection::open(&database_path)
            .map_err(|error| store_error("open query database", error))?;
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
            remove_pack_files(&root)?;
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
                 PRAGMA user_version=4;",
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
        let mut store = Self {
            connection,
            pack_path: root.join(active_pack),
            root,
            next_pack_generation,
        };
        store.remove_unreferenced_pack_files()?;
        Ok(store)
    }

    pub(crate) fn stage_output_digests(&self, query_key: &str) -> Result<Option<Vec<String>>> {
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
        if paths.len() != content_digests.len() {
            return Err(crate::Error::invalid(format!(
                "cached stage {stage:?} has {} paths for {} output digests",
                paths.len(),
                content_digests.len()
            )));
        }
        self.bind_stage(stage, query_key, paths, content_digests)
    }

    fn bind_stage(
        &mut self,
        stage: &str,
        query_key: &str,
        paths: &[String],
        content_digests: &[String],
    ) -> Result<()> {
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
        Ok(())
    }

    pub(crate) fn restore_output(&self, digest: &str, destination: &Path) -> Result<()> {
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
        Ok(result_digest)
    }

    pub(crate) fn get(&self, query_key: &str) -> Result<Option<Vec<u8>>> {
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
        Ok(Some(value))
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
        let mut pack = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&self.pack_path)?;
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
        self.index_objects(locations)
    }

    /// Append a batch before publishing any SQLite location. One durability
    /// barrier covers the whole batch; analysis workers never write SQLite or
    /// fsync once per function/output.
    fn ensure_objects<'a>(
        &mut self,
        values: impl IntoIterator<Item = (&'a str, &'a [u8])>,
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
        let mut pack = OpenOptions::new()
            .create(true)
            .append(true)
            .read(true)
            .open(&self.pack_path)?;
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
        self.index_objects(locations)
    }

    fn index_objects(&mut self, locations: Vec<(String, u64, u64)>) -> Result<()> {
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
        Ok(())
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
        let mut pack = File::open(self.root.join(pack_name))?;
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

    fn compact_if_needed(&mut self) -> Result<()> {
        let total_pack_bytes = pack_files(&self.root)?
            .into_iter()
            .try_fold(0_u64, |total, path| -> Result<u64> {
                Ok(total.saturating_add(fs::metadata(path)?.len()))
            })?;
        if total_pack_bytes < COMPACT_MIN_PACK_BYTES {
            return Ok(());
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
            || reclaimable.saturating_mul(4) < total_pack_bytes
        {
            return Ok(());
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
    fn compact(&mut self) -> Result<()> {
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
        let destination = self.root.join(&pack_name);
        let temporary = self.root.join(format!(
            ".{pack_name}.compact-{}-{}",
            std::process::id(),
            NEXT_RESTORE_ID.fetch_add(1, Ordering::Relaxed)
        ));
        let write_result = (|| -> Result<Vec<(String, u64, u64)>> {
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
            Ok(locations)
        })();
        let locations = match write_result {
            Ok(locations) => locations,
            Err(error) => {
                let _ = fs::remove_file(&temporary);
                return Err(error);
            }
        };
        fs::rename(&temporary, &destination)?;

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

        self.pack_path = destination;
        self.next_pack_generation = next_generation;
        self.remove_unreferenced_pack_files()
    }

    fn remove_unreferenced_pack_files(&mut self) -> Result<()> {
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
        for path in pack_files(&self.root)? {
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
        Ok(())
    }
}

fn pack_files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if path.is_file() && name.starts_with("objects-") && name.ends_with(".pack") {
            paths.push(path);
        }
    }
    paths.sort();
    Ok(paths)
}

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
}
