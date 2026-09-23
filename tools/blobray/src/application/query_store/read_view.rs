//! Shared lookup and CAS verification for readers and the analysis writer.
//!
//! A view borrows its owner's connection and file bindings. It neither opens a
//! database nor starts a transaction, publishes results, or restores outputs.
//! The owner supplies a fixed read transaction or a lifetime lock excluding
//! concurrent writers. A writer also uses this layer for its own lookups.

use super::*;

/// Pack access is chosen by the owner; a snapshot never falls back to a live
/// filename if its pinned descriptor is unavailable.
pub(super) enum PackAccess<'a> {
    Pinned(&'a BTreeMap<String, File>),
    Directory(&'a Path),
}

pub(super) struct ReadView<'a> {
    pub(super) connection: &'a Connection,
    pub(super) root: &'a Path,
    pub(super) root_identity: &'a CacheRootIdentity,
    pub(super) storage_root: &'a Path,
    pub(super) database_file: &'a File,
    pub(super) published_epoch: Option<&'a str>,
    pub(super) publishing_epoch: Option<&'a str>,
    pub(super) packs: PackAccess<'a>,
}

impl ReadView<'_> {
    pub(super) fn validated_stage_output_digests(
        &self,
        query_key: &str,
        validate_payloads: bool,
    ) -> Result<Option<Vec<String>>> {
        let digests = self.stage_output_digests(query_key)?;
        if validate_payloads && let Some(digests) = &digests {
            for digest in digests {
                self.validate_object_payload(digest)?;
            }
        }
        self.validate_root_identity()?;
        Ok(digests)
    }

    pub(super) fn validate_root_identity(&self) -> Result<()> {
        self.root_identity.validate(self.root)?;
        verify_open_file_path(
            self.database_file,
            &self.storage_root.join("queries.sqlite3"),
            "query cache database",
        )?;
        self.root_identity.validate(self.root)
    }

    pub(super) fn visible_epoch_sql_list(&self) -> Result<String> {
        let mut epochs = BTreeSet::from([standalone_epoch_id()]);
        if let Some(epoch) = self.published_epoch {
            validate_epoch_id(epoch)?;
            epochs.insert(epoch.to_owned());
        }
        if let Some(epoch) = self.publishing_epoch {
            validate_epoch_id(epoch)?;
            epochs.insert(epoch.to_owned());
        }
        Ok(epochs
            .into_iter()
            .map(|epoch| format!("'{epoch}'"))
            .collect::<Vec<_>>()
            .join(", "))
    }

    pub(super) fn stage_output_digests(&self, query_key: &str) -> Result<Option<Vec<String>>> {
        self.validate_root_identity()?;
        let visible_epochs = self.visible_epoch_sql_list()?;
        let kind = self
            .connection
            .query_row(
                &format!(
                    "SELECT result.kind FROM query_results AS result
                     WHERE result.query_key = ?1
                       AND EXISTS (
                           SELECT 1 FROM query_epoch_members AS member
                           WHERE member.query_key = result.query_key
                             AND member.epoch_id IN ({visible_epochs})
                       )"
                ),
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

    pub(super) fn get(&self, query_key: &str) -> Result<Option<Vec<u8>>> {
        self.validate_root_identity()?;
        let visible_epochs = self.visible_epoch_sql_list()?;
        let location = self
            .connection
            .query_row(
                &format!(
                    "SELECT result.result_digest, result.inline_value, result.object_digest
                     FROM query_results AS result
                     WHERE result.query_key = ?1
                       AND EXISTS (
                           SELECT 1 FROM query_epoch_members AS member
                           WHERE member.query_key = result.query_key
                             AND member.epoch_id IN ({visible_epochs})
                       )"
                ),
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

    pub(super) fn read_object(&self, digest: &str) -> Result<Vec<u8>> {
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

    pub(super) fn open_object(&self, digest: &str) -> Result<(crate::file_view::FileCursor, u64)> {
        let view = self.object_view(digest)?;
        Ok((view.cursor(), view.len()))
    }

    pub(super) fn object_view(&self, digest: &str) -> Result<crate::file_view::FileView> {
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
        let mut pack = match self.packs {
            PackAccess::Pinned(packs) => packs
                .get(&pack_name)
                .ok_or_else(|| {
                    crate::Error::invalid(format!(
                        "query snapshot has no pinned pack {pack_name:?}"
                    ))
                })?
                .try_clone()?,
            PackAccess::Directory(root) => open_cache_file_read_only(&root.join(&pack_name))?,
        };
        let metadata = pack.metadata()?;
        if !metadata.is_file() {
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
        crate::file_view::FileView::new(pack, offset + PACK_HEADER_BYTES, length)
    }
}
