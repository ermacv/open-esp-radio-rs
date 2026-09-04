//! One reachability projection shared by retention planning and publication.
//!
//! Epoch age is measured from retirement, not creation or completion. Epoch
//! pruning is explicit; ordinary compaction and object-only GC retain history.

use super::*;

#[cfg(test)]
mod tests;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum RetentionScope {
    #[default]
    RetiredObjects,
    RetiredEpochs,
}

fn projection_sql() -> String {
    format!(
        "WITH eligible_epochs AS (
             SELECT epoch.epoch_id FROM analysis_epochs AS epoch
             WHERE ?2 AND epoch.completed_unix_seconds IS NOT NULL
               AND epoch.retired_unix_seconds <= ?1
               AND epoch.epoch_id != '{}'
               AND epoch.epoch_id != (SELECT active_epoch FROM cache_state WHERE singleton = 1)
               AND NOT EXISTS (
                   SELECT 1 FROM epoch_pins AS pin WHERE pin.epoch_id = epoch.epoch_id
               )
         ), discarded_queries AS (
             SELECT result.query_key FROM query_results AS result
             WHERE EXISTS (
                 SELECT 1 FROM query_epoch_members AS member
                 WHERE member.query_key = result.query_key
                   AND member.epoch_id IN (SELECT epoch_id FROM eligible_epochs)
             ) AND NOT EXISTS (
                 SELECT 1 FROM query_epoch_members AS member
                 WHERE member.query_key = result.query_key
                   AND member.epoch_id NOT IN (SELECT epoch_id FROM eligible_epochs)
             )
         ), preserved_objects AS (
             SELECT object_digest AS digest FROM query_results
             WHERE object_digest IS NOT NULL
               AND query_key NOT IN (SELECT query_key FROM discarded_queries)
             UNION
             SELECT digest FROM stage_outputs
             WHERE epoch_id NOT IN (SELECT epoch_id FROM eligible_epochs)
             UNION
             SELECT digest FROM retired_objects
             WHERE ?1 IS NULL OR retired_unix_seconds > ?1
         ), expired_objects AS (
             SELECT digest FROM objects
             WHERE digest NOT IN (SELECT digest FROM preserved_objects)
               AND digest IN (
                   SELECT digest FROM retired_objects WHERE retired_unix_seconds <= ?1
                   UNION
                   SELECT object_digest FROM query_results
                   WHERE query_key IN (SELECT query_key FROM discarded_queries)
                   UNION
                   SELECT digest FROM stage_outputs
                   WHERE epoch_id IN (SELECT epoch_id FROM eligible_epochs)
               )
         ) ",
        standalone_epoch_id(),
    )
}

fn cutoff(value: Option<u64>) -> Result<Option<i64>> {
    value
        .map(|value| {
            i64::try_from(value)
                .map_err(|_| crate::Error::invalid("retention cutoff is outside SQLite INTEGER"))
        })
        .transpose()
}

fn validate_epoch_prune(
    connection: &Connection,
    cutoff: Option<i64>,
    scope: RetentionScope,
) -> Result<()> {
    if scope == RetentionScope::RetiredObjects {
        return Ok(());
    }
    let unscoped = query_nonnegative_count(
        connection,
        "validate epoch retention ownership",
        "SELECT COUNT(*) FROM query_results AS result WHERE NOT EXISTS (
             SELECT 1 FROM query_epoch_members AS member WHERE member.query_key = result.query_key
         )",
    )?;
    if unscoped != 0 {
        return Err(crate::Error::invalid(
            "epoch retention cannot repair query results without epoch ownership",
        ));
    }
    let invalid_bindings = query_nonnegative_count(
        connection,
        "validate retained stage ownership",
        "SELECT COUNT(*) FROM stage_bindings AS binding WHERE NOT EXISTS (
             SELECT 1 FROM query_epoch_members AS member
             WHERE member.query_key = binding.query_key AND member.epoch_id = binding.epoch_id
         )",
    )?;
    if invalid_bindings != 0 {
        return Err(crate::Error::invalid(
            "epoch retention cannot repair stage bindings outside their analysis epoch",
        ));
    }
    let dangling = connection
        .query_row(
            &(projection_sql()
                + "SELECT edge.query_key, edge.dependency_key FROM query_dependencies AS edge
             WHERE edge.query_key NOT IN (SELECT query_key FROM discarded_queries)
               AND edge.dependency_key IN (SELECT query_key FROM discarded_queries)
             ORDER BY edge.query_key, edge.dependency_key LIMIT 1"),
            params![cutoff, scope == RetentionScope::RetiredEpochs],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )
        .optional()
        .map_err(|error| store_error("validate retained query dependencies", error))?;
    if let Some((parent, child)) = dangling {
        return Err(crate::Error::invalid(format!(
            "epoch retention would remove the last owner of dependency {child:?} used by retained query {parent:?}; recompute the retained query with complete dependency ownership before pruning"
        )));
    }
    Ok(())
}

pub(super) fn measurements(
    connection: &Connection,
    cutoff_unix_seconds: u64,
    pack_bytes: u64,
    scope: RetentionScope,
) -> Result<RetentionMeasurements> {
    let cutoff = cutoff(Some(cutoff_unix_seconds))?;
    validate_epoch_prune(connection, cutoff, scope)?;
    let result = connection
        .query_row(
            &(projection_sql()
                + "SELECT
                 (SELECT COUNT(*) FROM retired_objects),
                 (SELECT COUNT(*) FROM eligible_epochs),
                 (SELECT COUNT(*) FROM discarded_queries),
                 (SELECT COUNT(*) FROM expired_objects),
                 (SELECT COALESCE(SUM(payload_length), 0) FROM objects
                  WHERE digest IN (SELECT digest FROM expired_objects)),
                 (SELECT COALESCE(SUM(?3 + payload_length), 0) FROM objects
                  WHERE digest IN (SELECT digest FROM expired_objects)),
                 (SELECT COALESCE(SUM(?3 + payload_length), 0) FROM objects
                  WHERE digest IN (SELECT digest FROM preserved_objects))"),
            params![
                cutoff,
                scope == RetentionScope::RetiredEpochs,
                PACK_HEADER_BYTES as i64
            ],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, i64>(3)?,
                    row.get::<_, i64>(4)?,
                    row.get::<_, i64>(5)?,
                    row.get::<_, i64>(6)?,
                ))
            },
        )
        .map_err(|error| store_error("measure query-cache retention projection", error))?;
    let measured = RetentionMeasurements {
        scope,
        retired_objects: nonnegative(result.0, "retired objects")?,
        eligible_epochs: nonnegative(result.1, "eligible epochs")?,
        eligible_queries: nonnegative(result.2, "eligible queries")?,
        eligible_objects: nonnegative(result.3, "eligible objects")?,
        eligible_payload_bytes: nonnegative(result.4, "eligible payload bytes")?,
        eligible_record_bytes: nonnegative(result.5, "eligible record bytes")?,
        projected_preserved_record_bytes: nonnegative(result.6, "preserved record bytes")?,
    };
    if measured.projected_preserved_record_bytes > pack_bytes {
        return Err(crate::Error::invalid(
            "retention projection preserves more bytes than the cache pack contains",
        ));
    }
    Ok(measured)
}

#[cfg(target_os = "linux")]
pub(super) fn preserved_digests(
    connection: &Connection,
    cutoff_unix_seconds: Option<u64>,
    scope: RetentionScope,
) -> Result<Vec<String>> {
    let cutoff = cutoff(cutoff_unix_seconds)?;
    validate_epoch_prune(connection, cutoff, scope)?;
    let mut statement = connection.prepare(&(projection_sql() +
        "SELECT digest FROM objects WHERE digest IN (SELECT digest FROM preserved_objects) ORDER BY digest"))
        .map_err(|error| store_error("prepare preserved retention objects", error))?;
    statement
        .query_map(
            params![cutoff, scope == RetentionScope::RetiredEpochs],
            |row| row.get(0),
        )
        .map_err(|error| store_error("read preserved retention objects", error))?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|error| store_error("decode preserved retention object", error))
}

#[cfg(target_os = "linux")]
pub(super) fn delete_epochs(
    transaction: &rusqlite::Transaction<'_>,
    cutoff_unix_seconds: Option<u64>,
    scope: RetentionScope,
) -> Result<()> {
    if scope == RetentionScope::RetiredObjects {
        return Ok(());
    }
    let cutoff = cutoff(cutoff_unix_seconds)?;
    validate_epoch_prune(transaction, cutoff, scope)?;
    transaction.execute(&(projection_sql() +
        "DELETE FROM query_results WHERE query_key IN (SELECT query_key FROM discarded_queries)"),
        params![cutoff, true])
        .map_err(|error| store_error("delete retired epoch query results", error))?;
    transaction.execute(&(projection_sql() +
        "DELETE FROM analysis_epochs WHERE epoch_id IN (SELECT epoch_id FROM eligible_epochs)"),
        params![cutoff, true])
        .map_err(|error| store_error("delete retired analysis epochs", error))?;
    Ok(())
}
