//! Read-only WAL snapshots, independent of the analysis writer capability.

use super::*;

/// A fixed SQLite read transaction plus the packs referenced by that snapshot.
///
/// Owns only read resources: the transaction, its published epoch, pinned
/// root/database identity and pack descriptors. Writer state and capabilities
/// are absent; lookup validation is shared through a borrowed `ReadView`.
pub(crate) struct QueryReader {
    // Drop the connection before releasing its file and directory handles.
    connection: PinnedConnection,
    root: PathBuf,
    root_identity: CacheRootIdentity,
    root_pin: PinnedCacheRoot,
    database_file: File,
    published_epoch: String,
    packs: BTreeMap<String, File>,
}

impl QueryReader {
    /// Open existing derived state without creating a database or cache root.
    /// SQLite owns WAL/SHM coordination; this does not acquire the writer flock.
    pub(crate) fn open(project_manifest: &Path) -> Result<Option<Self>> {
        Self::open_observed(project_manifest, || {})
    }

    fn open_observed(
        project_manifest: &Path,
        before_pack_pins: impl FnOnce(),
    ) -> Result<Option<Self>> {
        let root = project_manifest
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("generated/.blobray-cache");
        match fs::symlink_metadata(&root) {
            Ok(_) => {}
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
                ) =>
            {
                validate_absent_cache_root_parent(&root)?;
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        }
        let root_identity = CacheRootIdentity::capture(&root)?;
        let root_pin = PinnedCacheRoot::open(&root, &root_identity)?;
        let storage_root = root_pin.storage_root.clone();
        validate_cache_filesystem_for_wal(&storage_root)?;
        let database_path = storage_root.join("queries.sqlite3");
        match fs::symlink_metadata(&database_path) {
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                validate_cold_cache_root(&storage_root)?;
                root_identity.validate(&root)?;
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        }
        let database_file = open_cache_database_read_only(&database_path)?;
        validate_sqlite_sidecars(&database_path)?;

        // Destructive pack cleanup is Linux-only. A fresh description is
        // essential: duplicating root_pin's descriptor would share its flock.
        #[cfg(target_os = "linux")]
        let _pack_guard = {
            let directory = File::open(&storage_root)?;
            directory.lock_shared()?;
            AccessLock::new(directory)
        };
        let connection = Connection::open_with_flags(
            &database_path,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|error| store_error("open query snapshot", error))?;
        connection
            .set_db_config(DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE, true)
            .map_err(|error| store_error("disable snapshot checkpoint-on-close", error))?;
        connection
            .execute_batch("PRAGMA query_only=ON; BEGIN DEFERRED;")
            .map_err(|error| store_error("begin query snapshot", error))?;
        let schema: i64 = connection
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(|error| store_error("read snapshot schema", error))?;
        if schema == 0 {
            return Err(crate::Error::invalid(
                "query cache initialization is incomplete; retry after the initializing writer finishes",
            ));
        }
        if schema != STORE_SCHEMA {
            return Err(crate::Error::invalid(format!(
                "query cache schema {schema} is unsupported; expected {STORE_SCHEMA}; remove the disposable cache and rerun analysis"
            )));
        }
        let (active_pack, next_generation, published_epoch): (String, i64, String) = connection
            .query_row("SELECT active_pack, next_pack_generation, active_epoch FROM cache_state WHERE singleton = 1", [], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?))
            }).map_err(|error| store_error("read query snapshot state", error))?;
        if !is_pack_name(&active_pack) || next_generation < 0 {
            return Err(crate::Error::invalid(
                "query snapshot has invalid pack state",
            ));
        }
        validate_epoch_id(&published_epoch)?;
        validate_published_epoch(&connection, &published_epoch)?;
        before_pack_pins();
        let mut packs = BTreeMap::new();
        {
            let mut statement = connection
                .prepare("SELECT DISTINCT pack_name FROM objects")
                .map_err(|error| store_error("prepare snapshot pack pins", error))?;
            let names = statement
                .query_map([], |row| row.get::<_, String>(0))
                .map_err(|error| store_error("read snapshot pack names", error))?;
            for name in names {
                let name = name.map_err(|error| store_error("decode snapshot pack name", error))?;
                if !is_pack_name(&name) {
                    return Err(crate::Error::invalid(
                        "query snapshot references an invalid pack name",
                    ));
                }
                let file = open_cache_file_read_only(&storage_root.join(&name))?;
                packs.insert(name, file);
            }
        }
        let reader = Self {
            connection: PinnedConnection::read_only(connection),
            root,
            root_identity,
            root_pin,
            database_file,
            published_epoch,
            packs,
        };
        reader.read_view().validate_root_identity()?;
        Ok(Some(reader))
    }

    pub(crate) fn get(&self, query_key: &str) -> Result<Option<Vec<u8>>> {
        self.read_view().get(query_key)
    }

    pub(crate) fn published_epoch(&self) -> Option<&str> {
        (self.published_epoch != standalone_epoch_id()).then_some(&self.published_epoch)
    }

    pub(crate) fn output_view(
        &self,
        digest: &str,
        length: u64,
    ) -> Result<crate::file_view::FileView> {
        let key = crate::application::published_outputs::output_key(digest);
        let valid = self
            .connection
            .query_row(
                "SELECT result.kind = 'analysis-output' AND result.result_digest = ?2
                AND result.input_fingerprint = ?2 AND result.inline_value IS NULL
                AND result.object_digest = ?2
             FROM query_results AS result JOIN query_epoch_members AS member
                ON member.query_key = result.query_key
             WHERE result.query_key = ?1 AND member.epoch_id = ?3",
                params![key, digest, &self.published_epoch],
                |row| row.get::<_, bool>(0),
            )
            .optional()
            .map_err(|error| store_error("validate published output query", error))?;
        if valid != Some(true) {
            return Err(crate::Error::invalid(
                "published output has no matching retained query",
            ));
        }
        let view = self.read_view().object_view(digest)?;
        let identity = crate::application::generated_file::ContentIdentity::read(view.cursor())?;
        if identity.bytes != length || identity.sha256 != digest {
            return Err(crate::Error::invalid(
                "published output failed its content digest or length",
            ));
        }
        self.read_view().validate_root_identity()?;
        Ok(view)
    }

    fn read_view(&self) -> ReadView<'_> {
        ReadView {
            connection: &self.connection,
            root: &self.root,
            root_identity: &self.root_identity,
            storage_root: &self.root_pin.storage_root,
            database_file: &self.database_file,
            published_epoch: Some(&self.published_epoch),
            publishing_epoch: None,
            packs: PackAccess::Pinned(&self.packs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn absent_reader_does_not_create_state() {
        let manifest = super::super::tests::manifest("reader-absent");
        assert!(QueryReader::open(&manifest).unwrap().is_none());
        assert!(!manifest.parent().unwrap().join("generated").exists());
    }

    #[test]
    fn stage_lookup_retains_its_snapshot_across_publication_and_output_deletion() {
        use super::super::tests::{manifest, output};
        let manifest = manifest("reader-stage-publication");
        let first_output = output(&manifest, "first.json", b"first output");
        let mut writer = QueryStore::open_analysis_epoch(&manifest).unwrap();
        writer
            .record_stage(
                "fixture",
                "first-stage",
                std::slice::from_ref(&first_output),
            )
            .unwrap();
        writer.complete_analysis_epoch().unwrap();
        drop(writer);
        let reader = QueryReader::open(&manifest).unwrap().unwrap();
        fs::remove_file(&first_output.2).unwrap();
        let next_output = output(&manifest, "second.json", b"second output");
        let mut writer = QueryStore::open_analysis_epoch(&manifest).unwrap();
        writer
            .record_stage("fixture", "next-stage", std::slice::from_ref(&next_output))
            .unwrap();
        assert!(
            reader
                .read_view()
                .validated_stage_output_digests("next-stage", true)
                .unwrap()
                .is_none()
        );
        writer.complete_analysis_epoch().unwrap();
        assert_eq!(
            reader
                .read_view()
                .validated_stage_output_digests("first-stage", true)
                .unwrap(),
            Some(vec![first_output.1])
        );
        assert!(
            reader
                .read_view()
                .validated_stage_output_digests("next-stage", true)
                .unwrap()
                .is_none()
        );
        let fresh = QueryReader::open(&manifest).unwrap().unwrap();
        assert_eq!(
            fresh
                .read_view()
                .validated_stage_output_digests("next-stage", true)
                .unwrap(),
            Some(vec![next_output.1])
        );
        assert!(
            !first_output.2.exists(),
            "inspection must not restore a removed output"
        );
    }

    #[test]
    fn reader_keeps_published_snapshot_during_failed_and_successful_writers() {
        let manifest = super::super::tests::manifest("reader-epochs");
        let mut first = QueryStore::open_analysis_epoch(&manifest).unwrap();
        first
            .put("first", "fixture", "first", &[], b"published")
            .unwrap();
        first.complete_analysis_epoch().unwrap();
        drop(first);
        let reader = QueryReader::open(&manifest).unwrap().unwrap();
        assert!(
            reader
                .connection
                .execute("DELETE FROM query_results", [])
                .is_err()
        );
        let mut failed = QueryStore::open_analysis_epoch(&manifest).unwrap();
        failed
            .put("failed", "fixture", "failed", &[], b"private")
            .unwrap();
        assert_eq!(
            reader.get("first").unwrap().as_deref(),
            Some(b"published".as_slice())
        );
        let during_failure = QueryReader::open(&manifest).unwrap().unwrap();
        assert!(during_failure.get("failed").unwrap().is_none());
        drop(during_failure);
        // Shutdown leaves WAL frames pinned by the reader; it does not wait
        // for its own long-lived busy timeout or remove SQLite sidecars.
        drop(failed);
        assert_eq!(
            reader.get("first").unwrap().as_deref(),
            Some(b"published".as_slice())
        );
        let mut next = QueryStore::open_analysis_epoch(&manifest).unwrap();
        next.put("next", "fixture", "next", &[], b"new generation")
            .unwrap();
        assert!(
            QueryReader::open(&manifest)
                .unwrap()
                .unwrap()
                .get("next")
                .unwrap()
                .is_none()
        );
        next.complete_analysis_epoch().unwrap();
        assert!(reader.get("next").unwrap().is_none());
        assert_eq!(
            reader.get("first").unwrap().as_deref(),
            Some(b"published".as_slice())
        );
        let fresh = QueryReader::open(&manifest).unwrap().unwrap();
        assert_eq!(
            fresh.get("next").unwrap().as_deref(),
            Some(b"new generation".as_slice())
        );
        assert!(fresh.get("first").unwrap().is_none());
        assert!(fresh.get("failed").unwrap().is_none());
        drop(next);
        assert_eq!(
            fresh.get("next").unwrap().as_deref(),
            Some(b"new generation".as_slice())
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn stage_validation_uses_its_pinned_pack_and_detects_corruption_after_compaction() {
        use super::super::tests::{manifest, output};

        let manifest = manifest("reader-stage-compaction");
        let output = output(&manifest, "result.json", b"stage output");
        let mut writer = QueryStore::open(&manifest).unwrap();
        writer
            .record_stage("fixture", "stage", std::slice::from_ref(&output))
            .unwrap();
        let old_pack = writer.pack_path.clone();
        let mut old_file = OpenOptions::new().write(true).open(&old_pack).unwrap();
        let offset: i64 = writer
            .connection
            .query_row(
                "SELECT pack_offset FROM objects WHERE digest = ?1",
                [&output.1],
                |row| row.get(0),
            )
            .unwrap();
        let offset = u64::try_from(offset).unwrap();
        let reader = QueryReader::open(&manifest).unwrap().unwrap();
        old_file.seek(SeekFrom::End(0)).unwrap();
        old_file.write_all(b"unindexed tail").unwrap();
        writer.compact().unwrap();
        assert!(!old_pack.exists());
        fs::remove_file(&output.2).unwrap();
        let expected = Some(vec![output.1]);
        assert_eq!(
            reader
                .read_view()
                .validated_stage_output_digests("stage", true)
                .unwrap(),
            expected
        );

        // The old snapshot must validate its own bytes, even when a healthy
        // replacement pack with the same content digest exists in the index.
        old_file
            .seek(SeekFrom::Start(offset + PACK_HEADER_BYTES))
            .unwrap();
        old_file.write_all(b"!").unwrap();
        assert!(
            reader
                .read_view()
                .validated_stage_output_digests("stage", true)
                .unwrap_err()
                .to_string()
                .contains("failed its content digest")
        );
        assert_eq!(
            reader
                .read_view()
                .validated_stage_output_digests("stage", false)
                .unwrap(),
            expected
        );
        let fresh = QueryReader::open(&manifest).unwrap().unwrap();
        assert_eq!(
            fresh
                .read_view()
                .validated_stage_output_digests("stage", true)
                .unwrap(),
            expected
        );
        assert!(!output.2.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pinned_reader_keeps_payload_after_compaction_unlinks_its_pack() {
        let manifest = super::super::tests::manifest("reader-compaction");
        let mut writer = QueryStore::open(&manifest).unwrap();
        let bytes = vec![0x73; INLINE_VALUE_LIMIT + 1];
        writer
            .put("packed", "fixture", "packed", &[], &bytes)
            .unwrap();
        let old_pack = writer.pack_path.clone();
        let reader = QueryReader::open(&manifest).unwrap().unwrap();
        OpenOptions::new()
            .append(true)
            .open(&old_pack)
            .unwrap()
            .write_all(b"unreferenced tail")
            .unwrap();
        writer.compact().unwrap();
        assert!(!old_pack.exists());
        assert_eq!(reader.get("packed").unwrap().unwrap(), bytes);
        writer
            .put("later", "fixture", "later", &[], b"standalone change")
            .unwrap();
        assert!(reader.get("later").unwrap().is_none());
        assert_eq!(
            QueryReader::open(&manifest)
                .unwrap()
                .unwrap()
                .get("later")
                .unwrap()
                .as_deref(),
            Some(b"standalone change".as_slice())
        );
        drop(writer);
        assert_eq!(reader.get("packed").unwrap().unwrap(), bytes);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn compaction_during_reader_open_defers_unlink_until_pack_is_pinned() {
        let manifest = super::super::tests::manifest("reader-pinning-race");
        let mut writer = QueryStore::open(&manifest).unwrap();
        let bytes = vec![0x26; INLINE_VALUE_LIMIT + 1];
        writer
            .put("packed", "fixture", "packed", &[], &bytes)
            .unwrap();
        let old_pack = writer.pack_path.clone();
        OpenOptions::new()
            .append(true)
            .open(&old_pack)
            .unwrap()
            .write_all(b"unindexed bytes")
            .unwrap();
        let reader = QueryReader::open_observed(&manifest, || {
            writer.compact().unwrap();
            assert_ne!(writer.pack_path, old_pack);
            assert!(
                old_pack.exists(),
                "opening reader still needs to pin its old snapshot's pack"
            );
        })
        .unwrap()
        .unwrap();
        writer.remove_unreferenced_pack_files().unwrap();
        assert!(!old_pack.exists());
        assert_eq!(reader.get("packed").unwrap().unwrap(), bytes);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn replaced_root_cannot_redirect_a_reader_to_another_generation() {
        let manifest = super::super::tests::manifest("reader-root-replacement");
        let mut writer = QueryStore::open(&manifest).unwrap();
        writer.put("key", "fixture", "old", &[], b"old").unwrap();
        let root = writer.root.clone();
        drop(writer);
        let reader = QueryReader::open(&manifest).unwrap().unwrap();
        fs::rename(&root, root.with_file_name("retired-reader-root")).unwrap();
        let mut writer = QueryStore::open(&manifest).unwrap();
        writer.put("key", "fixture", "new", &[], b"new").unwrap();
        assert!(
            reader
                .get("key")
                .unwrap_err()
                .to_string()
                .contains("was replaced")
        );
        drop(reader);
        assert_eq!(
            writer.read_view().get("key").unwrap().as_deref(),
            Some(b"new".as_slice())
        );
    }
}
