use super::*;
use crate::analysis::FunctionFactStore;
use crate::application::query_store::tests::{manifest, output, snapshot_tree};

fn age_retired_epochs(store: &QueryStore) {
    store
        .connection
        .execute(
            "UPDATE analysis_epochs SET created_unix_seconds = 1,
             completed_unix_seconds = 2, retired_unix_seconds = 3
         WHERE retired_unix_seconds IS NOT NULL",
            [],
        )
        .unwrap();
}

#[cfg(target_os = "linux")]
#[test]
fn explicit_epoch_prune_preserves_current_pins_shared_facts_and_evidence() {
    let manifest = manifest("epoch-retention-protected-roots");
    let evidence = output(
        &manifest,
        "revisions/baseline.toml",
        b"durable reviewed evidence",
    );
    let shared = (
        "function-direct:shared".to_owned(),
        vec![0x77; INLINE_VALUE_LIMIT + 1],
    );
    let mut epochs = BTreeMap::new();
    let mut digests = BTreeMap::new();
    for (index, (label, pin)) in [
        ("expired", None),
        ("manual", Some("manual")),
        ("baseline", Some("revision-baseline")),
        ("pinned-current", Some("revision-current")),
        ("recent", None),
        ("current", None),
    ]
    .into_iter()
    .enumerate()
    {
        let mut store = QueryStore::open_analysis_epoch(&manifest).unwrap();
        let epoch = store.writable_active_epoch().unwrap().to_owned();
        epochs.insert(label, epoch.clone());
        store.begin_stage_queries("linked-ir");
        let fact = (
            format!("function-direct:{label}"),
            vec![index as u8; INLINE_VALUE_LIMIT + 1],
        );
        digests.insert(label, sha256_hex(&fact.1));
        store.store_function_facts(&[fact]).unwrap();
        if matches!(label, "expired" | "current") {
            store
                .store_function_facts(std::slice::from_ref(&shared))
                .unwrap();
        }
        let cached = output(
            &manifest,
            &format!("generated/{label}.json"),
            label.as_bytes(),
        );
        store
            .record_stage("linked-ir", &format!("stage:{label}"), &[cached])
            .unwrap();
        if let Some(kind) = pin {
            store
                .connection
                .execute(
                    "INSERT INTO epoch_pins(pin_id, epoch_id, kind) VALUES (?1, ?2, ?3)",
                    params![label, epoch, kind],
                )
                .unwrap();
        }
        store.complete_analysis_epoch().unwrap();
    }
    {
        let mut store = QueryStore::open(&manifest).unwrap();
        age_retired_epochs(&store);
        // This epoch was created long ago but retired recently. Its age must
        // be measured from retirement, not its creation or completion.
        store
            .connection
            .execute(
                "UPDATE analysis_epochs SET retired_unix_seconds = ?2 WHERE epoch_id = ?1",
                params![epochs["recent"], unix_timestamp_seconds().unwrap() as i64],
            )
            .unwrap();
        store
            .put("focused", "function", "focused", &[], b"standalone result")
            .unwrap();
    }
    {
        let mut failed = QueryStore::open_analysis_epoch(&manifest).unwrap();
        failed
            .put("failed", "function", "failed", &[], b"unpublished")
            .unwrap();
    }
    let before = snapshot_tree(manifest.parent().unwrap());
    let duration = Duration::from_secs(30 * 24 * 60 * 60);
    let default_plan =
        QueryStore::retention_plan(&manifest, duration, None, RetentionScope::RetiredObjects)
            .unwrap();
    assert!(
        !default_plan.would_prune,
        "default GC must retain successful history"
    );
    let plan = QueryStore::retention_plan(&manifest, duration, None, RetentionScope::RetiredEpochs)
        .unwrap();
    assert_eq!(snapshot_tree(manifest.parent().unwrap()), before);
    assert_eq!(plan.eligible_epochs, 1);
    assert_eq!(
        plan.eligible_queries, 2,
        "only the expired private fact and stage lose all owners"
    );
    assert_eq!(plan.eligible_objects, 2);
    assert!(plan.ready_to_prune);

    let result =
        QueryStore::prune_cache(&manifest, duration, None, RetentionScope::RetiredEpochs).unwrap();

    assert_eq!(
        (
            result.pruned_epochs,
            result.pruned_queries,
            result.pruned_objects
        ),
        (1, 2, 2)
    );
    let mut store = QueryStore::open(&manifest).unwrap();
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM analysis_epochs WHERE epoch_id = ?1",
                [&epochs["expired"]],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    for label in ["manual", "baseline", "pinned-current", "recent", "current"] {
        assert_eq!(
            store
                .connection
                .query_row(
                    "SELECT COUNT(*) FROM analysis_epochs WHERE epoch_id = ?1",
                    [&epochs[label]],
                    |row| row.get::<_, i64>(0)
                )
                .unwrap(),
            1
        );
        assert!(store.open_object(&digests[label]).is_ok());
    }
    assert!(store.open_object(&digests["expired"]).is_err());
    assert!(store.get("function-direct:expired").unwrap().is_none());
    assert!(
        store.get("function-direct:baseline").unwrap().is_none(),
        "pinning must not promote historical visibility"
    );
    assert!(
        store.get("failed").unwrap().is_none(),
        "GC must not publish failed-epoch results"
    );
    assert_eq!(
        store
            .connection
            .query_row(
                "SELECT COUNT(*) FROM query_results WHERE query_key = 'failed'",
                [],
                |row| row.get::<_, i64>(0)
            )
            .unwrap(),
        1
    );
    assert_eq!(
        store.get("focused").unwrap(),
        Some(b"standalone result".to_vec())
    );
    assert_eq!(
        store
            .load_function_facts(std::slice::from_ref(&shared.0))
            .unwrap(),
        vec![shared]
    );
    // The surviving stage still restores its complete function dependency closure.
    store
        .bind_restored_stage(
            "linked-ir:restored",
            "stage:current",
            &["generated/restored.json".to_owned()],
            &[sha256_hex(b"current")],
        )
        .unwrap();
    assert_eq!(fs::read(evidence.2).unwrap(), b"durable reviewed evidence");
    assert_eq!(
        fs::read(manifest.parent().unwrap().join("generated/expired.json")).unwrap(),
        b"expired"
    );
}

#[cfg(target_os = "linux")]
#[test]
fn inline_only_history_can_be_pruned_without_any_cas_candidates() {
    let manifest = manifest("epoch-retention-inline");
    {
        let mut first = QueryStore::open_analysis_epoch(&manifest).unwrap();
        first
            .put("old", "function", "old", &[], b"old inline")
            .unwrap();
        first.complete_analysis_epoch().unwrap();
    }
    {
        let mut current = QueryStore::open_analysis_epoch(&manifest).unwrap();
        current
            .put("current", "function", "current", &[], b"current inline")
            .unwrap();
        current.complete_analysis_epoch().unwrap();
        age_retired_epochs(&current);
    }
    let plan = QueryStore::retention_plan(
        &manifest,
        Duration::ZERO,
        None,
        RetentionScope::RetiredEpochs,
    )
    .unwrap();
    assert_eq!(
        (
            plan.eligible_epochs,
            plan.eligible_queries,
            plan.eligible_objects
        ),
        (1, 1, 0)
    );
    assert!(plan.ready_to_prune);
    assert!(plan.maintenance.temporary_bytes_required >= COMPACT_FREE_SPACE_RESERVE_BYTES);
    let result = QueryStore::prune_cache(
        &manifest,
        Duration::ZERO,
        None,
        RetentionScope::RetiredEpochs,
    )
    .unwrap();
    assert_eq!(
        (
            result.pruned_epochs,
            result.pruned_queries,
            result.pruned_objects
        ),
        (1, 1, 0)
    );
    let store = QueryStore::open(&manifest).unwrap();
    assert!(
        store
            .connection
            .query_row(
                "SELECT query_key FROM query_results WHERE query_key = 'old'",
                [],
                |row| row.get::<_, String>(0)
            )
            .optional()
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.get("current").unwrap(),
        Some(b"current inline".to_vec())
    );
}

#[cfg(target_os = "linux")]
#[test]
fn epoch_retention_quota_refusal_preserves_metadata_and_files() {
    let manifest = manifest("epoch-retention-quota");
    for key in ["old", "current"] {
        let mut store = QueryStore::open_analysis_epoch(&manifest).unwrap();
        store
            .put(key, "function", key, &[], key.as_bytes())
            .unwrap();
        store.complete_analysis_epoch().unwrap();
    }
    let before = snapshot_tree(manifest.parent().unwrap());
    let error = QueryStore::prune_cache(
        &manifest,
        Duration::ZERO,
        Some(1),
        RetentionScope::RetiredEpochs,
    )
    .unwrap_err();
    assert!(error.to_string().contains("over --max-size"));
    assert_eq!(snapshot_tree(manifest.parent().unwrap()), before);
}

#[test]
fn epoch_retention_fails_closed_on_missing_retained_dependency_ownership() {
    let manifest = manifest("epoch-retention-dependency-ownership");
    {
        let mut first = QueryStore::open_analysis_epoch(&manifest).unwrap();
        first
            .put("child", "function", "child", &[], b"child")
            .unwrap();
        first.complete_analysis_epoch().unwrap();
    }
    {
        let mut current = QueryStore::open_analysis_epoch(&manifest).unwrap();
        // A generic query writer skipped consumption/closure publication.
        // GC must refuse, rather than delete the child or promote hidden data.
        current
            .put(
                "parent",
                "function",
                "parent",
                &["child".to_owned()],
                b"parent",
            )
            .unwrap();
        current.complete_analysis_epoch().unwrap();
    }
    let before = snapshot_tree(manifest.parent().unwrap());
    let error = QueryStore::prune_cache(
        &manifest,
        Duration::ZERO,
        None,
        RetentionScope::RetiredEpochs,
    )
    .unwrap_err();
    assert!(error.to_string().contains("last owner of dependency"));
    assert_eq!(snapshot_tree(manifest.parent().unwrap()), before);
    let store = QueryStore::open(&manifest).unwrap();
    assert!(store.get("child").unwrap().is_none());
}

#[cfg(target_os = "linux")]
#[test]
fn epoch_metadata_is_not_deleted_when_preserved_cas_verification_fails() {
    let manifest = manifest("epoch-retention-corrupt-live-cas");
    {
        let mut first = QueryStore::open_analysis_epoch(&manifest).unwrap();
        first.put("old", "function", "old", &[], b"old").unwrap();
        first.complete_analysis_epoch().unwrap();
    }
    let (pack, offset) = {
        let mut current = QueryStore::open_analysis_epoch(&manifest).unwrap();
        current
            .put(
                "current",
                "function",
                "current",
                &[],
                &vec![0x31; INLINE_VALUE_LIMIT + 1],
            )
            .unwrap();
        current.complete_analysis_epoch().unwrap();
        let offset = current
            .connection
            .query_row("SELECT pack_offset FROM objects LIMIT 1", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap();
        (current.pack_path.clone(), offset as u64 + PACK_HEADER_BYTES)
    };
    use std::io::Write;
    let mut file = OpenOptions::new().write(true).open(pack).unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(b"X").unwrap();
    drop(file);
    let before = QueryStore::statistics(&manifest).unwrap();
    let error = QueryStore::prune_cache(
        &manifest,
        Duration::ZERO,
        None,
        RetentionScope::RetiredEpochs,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("failed its digest during compaction")
    );
    let after = QueryStore::statistics(&manifest).unwrap();
    assert_eq!(after.analysis_epochs, before.analysis_epochs);
    assert_eq!(after.query_results, before.query_results);
    assert_eq!(after.active_epoch, before.active_epoch);
}

#[test]
fn metadata_only_epoch_prune_reserves_space_before_any_mutation() {
    let mut statistics =
        QueryStoreStatistics::empty("cache".into(), "cache/queries.sqlite3".into());
    statistics.present = true;
    statistics.database_bytes = 1_000;
    statistics.root_bytes = 2_000;
    statistics.pack_bytes = 1_000;
    let measurements = RetentionMeasurements {
        scope: RetentionScope::RetiredEpochs,
        eligible_epochs: 1,
        eligible_queries: 1,
        retired_objects: 0,
        eligible_objects: 0,
        eligible_payload_bytes: 0,
        eligible_record_bytes: 0,
        projected_preserved_record_bytes: 1_000,
    };
    let plan = build_retention_plan(
        &statistics,
        CacheFilesystemAssessment {
            supported: true,
            kind: "local".to_owned(),
            magic: None,
            available_bytes: Some(1_999),
        },
        None,
        30,
        100,
        measurements,
    )
    .unwrap();
    assert!(plan.would_prune);
    assert!(!plan.ready_to_prune);
    assert!(plan.maintenance.temporary_bytes_required > 2_000);
    assert!(!plan.maintenance.enough_free_space);
}

#[test]
fn epoch_retention_cutoff_includes_equal_retirement_timestamps() {
    let manifest = manifest("epoch-retention-cutoff");
    let mut epochs = Vec::new();
    for key in ["at-cutoff", "after-cutoff", "current"] {
        let mut store = QueryStore::open_analysis_epoch(&manifest).unwrap();
        epochs.push(store.writable_active_epoch().unwrap().to_owned());
        store
            .put(key, "function", key, &[], key.as_bytes())
            .unwrap();
        store.complete_analysis_epoch().unwrap();
    }
    {
        let store = QueryStore::open(&manifest).unwrap();
        for (epoch, retired) in [(&epochs[0], 10), (&epochs[1], 11)] {
            store
                .connection
                .execute(
                    "UPDATE analysis_epochs SET created_unix_seconds = 1,
                     completed_unix_seconds = 2, retired_unix_seconds = ?2
                 WHERE epoch_id = ?1",
                    params![epoch, retired],
                )
                .unwrap();
        }
    }
    let plan =
        QueryStore::retention_plan_at(&manifest, 30, 10, None, RetentionScope::RetiredEpochs)
            .unwrap();
    assert_eq!((plan.eligible_epochs, plan.eligible_queries), (1, 1));
    let plan =
        QueryStore::retention_plan_at(&manifest, 30, 11, None, RetentionScope::RetiredEpochs)
            .unwrap();
    assert_eq!((plan.eligible_epochs, plan.eligible_queries), (2, 2));
}
