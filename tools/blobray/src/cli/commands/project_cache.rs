//! Inspection and explicit maintenance of the project-owned query cache.

use std::{path::Path, time::Duration};

use crate::{
    Result,
    application::{
        ProjectCacheCompactionResult, ProjectCacheMaintenancePlan, ProjectCachePruneResult,
        ProjectCacheRetentionPlan, ProjectCacheStatistics, compact_project_cache,
        project_cache_maintenance_plan, project_cache_retention_plan, project_cache_statistics,
        prune_project_cache,
    },
    cli::{ProjectCacheCompactArgs, ProjectCacheGcArgs, output, table},
};

#[derive(serde::Serialize)]
struct CacheStatsDocument<'a> {
    schema_version: u32,
    command: &'static str,
    #[serde(flatten)]
    statistics: &'a ProjectCacheStatistics,
}

#[derive(serde::Serialize)]
struct CacheGcDocument<'a> {
    schema_version: u32,
    command: &'static str,
    dry_run: bool,
    #[serde(flatten)]
    plan: &'a ProjectCacheMaintenancePlan,
}

#[derive(serde::Serialize)]
struct CacheCompactDocument<'a> {
    schema_version: u32,
    command: &'static str,
    #[serde(flatten)]
    result: &'a ProjectCacheCompactionResult,
}

#[derive(serde::Serialize)]
struct CacheRetentionPlanDocument<'a> {
    schema_version: u32,
    command: &'static str,
    dry_run: bool,
    operation: &'static str,
    plan: &'a ProjectCacheRetentionPlan,
}

#[derive(serde::Serialize)]
struct CachePruneDocument<'a> {
    schema_version: u32,
    command: &'static str,
    dry_run: bool,
    operation: &'static str,
    result: &'a ProjectCachePruneResult,
}

pub(super) fn stats(project_manifest: &Path) -> Result<bool> {
    let statistics = project_cache_statistics(project_manifest)?;
    let document = CacheStatsDocument {
        schema_version: 3,
        command: "project cache stats",
        statistics: &statistics,
    };
    output::render_report(&document, || render_stats_human(&statistics));
    Ok(true)
}

pub(super) fn gc(arguments: ProjectCacheGcArgs, project_manifest: &Path) -> Result<bool> {
    if arguments.dry_run == arguments.apply {
        return Err(crate::Error::invalid(
            "project cache gc requires exactly one of --dry-run or --apply",
        ));
    }
    if arguments.apply && arguments.retention_days.is_none() {
        return Err(crate::Error::invalid(
            "project cache gc --apply requires --retention-days; current/live results are never age-eviction candidates",
        ));
    }
    if let Some(retention_days) = arguments.retention_days {
        let retention = retention_duration(retention_days)?;
        if arguments.apply {
            let result = prune_project_cache(project_manifest, retention, arguments.max_size)?;
            let document = CachePruneDocument {
                schema_version: 1,
                command: "project cache gc",
                dry_run: false,
                operation: "retention-prune",
                result: &result,
            };
            output::render_report(&document, || render_prune_result(&result));
        } else {
            let plan =
                project_cache_retention_plan(project_manifest, retention, arguments.max_size)?;
            let document = CacheRetentionPlanDocument {
                schema_version: 1,
                command: "project cache gc",
                dry_run: true,
                operation: "retention-prune",
                plan: &plan,
            };
            output::render_report(&document, || render_retention_plan(&plan));
        }
        return Ok(true);
    }
    let plan = project_cache_maintenance_plan(project_manifest, arguments.max_size)?;
    let document = CacheGcDocument {
        schema_version: 1,
        command: "project cache gc",
        dry_run: true,
        plan: &plan,
    };
    output::render_report(&document, || render_maintenance_plan(&plan));
    Ok(true)
}

fn retention_duration(days: u64) -> Result<Duration> {
    let seconds = days
        .checked_mul(24 * 60 * 60)
        .ok_or_else(|| crate::Error::invalid("cache retention duration overflowed u64"))?;
    Ok(Duration::from_secs(seconds))
}

pub(super) fn compact(arguments: ProjectCacheCompactArgs, project_manifest: &Path) -> Result<bool> {
    let result = compact_project_cache(project_manifest, arguments.max_size)?;
    let document = CacheCompactDocument {
        schema_version: 1,
        command: "project cache compact",
        result: &result,
    };
    output::render_report(&document, || render_compaction_result(&result));
    Ok(true)
}

fn render_stats_human(statistics: &ProjectCacheStatistics) {
    outputln!("{}", output::heading("Project cache"));
    if !statistics.present {
        outputln!(
            "\n{}",
            output::warning(format!(
                "NOT CREATED — no cache exists at {}",
                statistics.cache_root.display()
            ))
        );
        outputln!("The cache is created lazily by a writing project analysis.");
        return;
    }

    outputln!(
        "\n{}",
        output::success(format!(
            "CACHE PRESENT — {}, {} queries, {} dependencies, {} reclaimable",
            human_bytes(statistics.root_bytes),
            statistics.query_results,
            statistics.dependencies,
            human_bytes(statistics.reclaimable_pack_bytes),
        ))
    );

    outputln!("\n{}", output::heading("Assessment"));
    if !statistics.compaction.supported {
        outputln!(
            "- Automatic pack compaction is disabled on this platform because the cache root cannot yet be pinned strongly enough for destructive cleanup."
        );
    } else if statistics.compaction.eligible_on_next_write {
        outputln!(
            "- Pack compaction is eligible and will run automatically after the next successful cache write."
        );
    } else {
        outputln!(
            "- Pack compaction is not needed: it requires at least {} total, {} reclaimable and {}% garbage.",
            human_bytes(statistics.compaction.minimum_pack_bytes),
            human_bytes(statistics.compaction.minimum_reclaimable_bytes),
            statistics.compaction.minimum_reclaimable_percent,
        );
    }
    outputln!(
        "- This directory is disposable local acceleration state. Preserve research with reviewed TOML, durable revision snapshots and reproducible linked IR, not by copying the cache."
    );
    outputln!(
        "- Use `project analyze --plan` to classify stages as current, restorable or recomputed before a write."
    );
    outputln!(
        "\n{}",
        table::render(
            ["Metric", "Value"],
            [
                ["Cache files".to_owned(), human_bytes(statistics.root_bytes)],
                [
                    "Database".to_owned(),
                    human_bytes(statistics.database_bytes)
                ],
                ["Pack files".to_owned(), human_bytes(statistics.pack_bytes)],
                [
                    "Query results".to_owned(),
                    statistics.query_results.to_string()
                ],
                [
                    "Dependencies".to_owned(),
                    statistics.dependencies.to_string()
                ],
                ["Objects".to_owned(), statistics.objects.to_string()],
                [
                    "Reclaimable pack".to_owned(),
                    human_bytes(statistics.reclaimable_pack_bytes),
                ],
            ],
        )
    );

    if !output::details() {
        return;
    }

    outputln!("\n{}", output::heading("Paths"));
    outputln!(
        "{}",
        table::render(
            ["Role", "Path"],
            [
                [
                    "Cache root".to_owned(),
                    statistics.cache_root.display().to_string(),
                ],
                [
                    "SQLite database".to_owned(),
                    statistics.database_path.display().to_string(),
                ],
            ],
        )
    );

    outputln!("\n{}", output::heading("Stored data"));
    outputln!(
        "{}",
        table::render(["Metric", "Value"], detail_metric_rows(statistics))
    );

    if !statistics.query_kinds.is_empty() {
        outputln!("\n{}", output::heading("Query kinds"));
        outputln!(
            "{}",
            table::render(
                ["Kind", "Queries", "Inline data"],
                statistics.query_kinds.iter().map(|kind| [
                    kind.kind.clone(),
                    kind.query_results.to_string(),
                    human_bytes(kind.inline_bytes),
                ]),
            )
        );
    }
}

fn render_maintenance_plan(plan: &ProjectCacheMaintenancePlan) {
    outputln!("{}", output::heading("Project cache GC dry run"));
    let status = if plan.ready_to_compact {
        output::success(format!(
            "READY — would reclaim {} and project {}",
            human_bytes(plan.reclaimable_bytes),
            human_bytes(plan.projected_root_bytes),
        ))
    } else if let Some(reason) = plan.reason.as_deref() {
        output::warning(format!("NO MUTATION — {reason}"))
    } else {
        output::warning("NO MUTATION")
    };
    outputln!("\n{status}");
    outputln!(
        "\n{}",
        table::render(["Metric", "Value"], maintenance_rows(plan),)
    );
}

fn render_compaction_result(result: &ProjectCacheCompactionResult) {
    outputln!("{}", output::heading("Project cache compaction"));
    if result.compacted {
        outputln!(
            "\n{}",
            output::success(format!(
                "COMPACTED — reclaimed {}, final size {}",
                human_bytes(result.reclaimed_bytes),
                human_bytes(result.final_root_bytes),
            ))
        );
    } else {
        outputln!(
            "\n{}",
            output::warning(format!(
                "NO CHANGE — {}",
                result
                    .plan
                    .reason
                    .as_deref()
                    .unwrap_or("no unreachable pack bytes"),
            ))
        );
    }
}

fn render_retention_plan(plan: &ProjectCacheRetentionPlan) {
    outputln!("{}", output::heading("Project cache retention dry run"));
    let status = if plan.ready_to_prune {
        output::success(format!(
            "READY — {} retired objects are older than the cutoff; would reclaim {}",
            plan.eligible_objects,
            human_bytes(plan.maintenance.reclaimable_bytes),
        ))
    } else {
        output::warning(format!(
            "NO MUTATION — {}",
            plan.reason
                .as_deref()
                .unwrap_or("no eligible retired objects")
        ))
    };
    outputln!("\n{status}");
    outputln!(
        "\n{}",
        table::render(["Metric", "Value"], retention_rows(plan))
    );
    outputln!(
        "\nCurrent query results and stage outputs are hard roots; retention and --max-size never evict them."
    );
}

fn render_prune_result(result: &ProjectCachePruneResult) {
    outputln!("{}", output::heading("Project cache retention prune"));
    if result.compacted {
        outputln!(
            "\n{}",
            output::success(format!(
                "PRUNED — removed {} retired objects, reclaimed {}, final size {}",
                result.pruned_objects,
                human_bytes(result.reclaimed_bytes),
                human_bytes(result.final_root_bytes),
            ))
        );
    } else {
        outputln!(
            "\n{}",
            output::warning(format!(
                "NO CHANGE — {}",
                result
                    .plan
                    .reason
                    .as_deref()
                    .unwrap_or("no eligible retired objects")
            ))
        );
    }
}

fn retention_rows(plan: &ProjectCacheRetentionPlan) -> Vec<[String; 2]> {
    let mut rows = vec![
        [
            "Retention age".to_owned(),
            format!("{} days", plan.retention_seconds / (24 * 60 * 60)),
        ],
        [
            "Retired objects".to_owned(),
            plan.retired_objects.to_string(),
        ],
        [
            "Eligible objects".to_owned(),
            plan.eligible_objects.to_string(),
        ],
        [
            "Eligible payload".to_owned(),
            human_bytes(plan.eligible_payload_bytes),
        ],
    ];
    rows.extend(maintenance_rows(&plan.maintenance));
    rows
}

fn maintenance_rows(plan: &ProjectCacheMaintenancePlan) -> Vec<[String; 2]> {
    let mut rows = vec![
        ["Filesystem".to_owned(), plan.filesystem.clone()],
        ["Current cache".to_owned(), human_bytes(plan.root_bytes)],
        [
            "Reclaimable".to_owned(),
            human_bytes(plan.reclaimable_bytes),
        ],
        [
            "Projected cache".to_owned(),
            human_bytes(plan.projected_root_bytes),
        ],
        [
            "Temporary space required".to_owned(),
            human_bytes(plan.temporary_bytes_required),
        ],
        [
            "Available space".to_owned(),
            plan.available_bytes
                .map_or_else(|| "unknown".to_owned(), human_bytes),
        ],
    ];
    if let Some(max_size) = plan.max_size_bytes {
        rows.push(["Maximum size".to_owned(), human_bytes(max_size)]);
        rows.push([
            "Over limit after compaction".to_owned(),
            human_bytes(plan.over_max_size_bytes),
        ]);
    }
    rows
}

fn detail_metric_rows(statistics: &ProjectCacheStatistics) -> Vec<[String; 2]> {
    vec![
        [
            "Store schema".to_owned(),
            statistics
                .schema
                .map_or_else(|| "unknown".to_owned(), |schema| schema.to_string()),
        ],
        [
            "Inline query data".to_owned(),
            human_bytes(statistics.inline_bytes),
        ],
        [
            "Object payload".to_owned(),
            human_bytes(statistics.object_payload_bytes),
        ],
        [
            "Stage bindings".to_owned(),
            statistics.stage_bindings.to_string(),
        ],
        [
            "Stage outputs".to_owned(),
            statistics.stage_outputs.to_string(),
        ],
        [
            "Live objects".to_owned(),
            statistics.live_objects.to_string(),
        ],
        [
            "Live pack records".to_owned(),
            human_bytes(statistics.live_record_bytes),
        ],
        [
            "Retired CAS objects".to_owned(),
            statistics.retired_objects.to_string(),
        ],
        [
            "Retired CAS records".to_owned(),
            human_bytes(statistics.retired_record_bytes),
        ],
        [
            "Preserved pack records".to_owned(),
            human_bytes(statistics.preserved_record_bytes),
        ],
    ]
}

fn human_bytes(bytes: u64) -> String {
    const UNITS: [(&str, u64); 4] = [
        ("GiB", 1024 * 1024 * 1024),
        ("MiB", 1024 * 1024),
        ("KiB", 1024),
        ("B", 1),
    ];
    let (unit, scale) = UNITS
        .into_iter()
        .find(|(_, scale)| bytes >= *scale)
        .unwrap_or(("B", 1));
    let whole = bytes / scale;
    let decimal = bytes.saturating_sub(whole * scale).saturating_mul(10) / scale;
    if scale == 1 || whole >= 10 || decimal == 0 {
        format!("{whole} {unit}")
    } else {
        format!("{whole}.{decimal} {unit}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_sizes_are_compact_and_stable() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(1024), "1 KiB");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(12 * 1024 * 1024), "12 MiB");
    }

    #[test]
    fn machine_document_has_command_identity_separate_from_store_schema() {
        let statistics = ProjectCacheStatistics::empty(
            "generated/.blobray-cache".into(),
            "generated/.blobray-cache/queries.sqlite3".into(),
        );
        let document = serde_json::to_value(CacheStatsDocument {
            schema_version: 3,
            command: "project cache stats",
            statistics: &statistics,
        })
        .unwrap();
        assert_eq!(document["schema_version"], 3);
        assert_eq!(document["command"], "project cache stats");
        assert_eq!(document["schema"], serde_json::Value::Null);
        assert_eq!(document["present"], false);
        assert_eq!(document["compaction"]["eligible_on_next_write"], false);
        assert_eq!(detail_metric_rows(&statistics).len(), 10);
        assert_eq!(detail_metric_rows(&statistics)[0][0], "Store schema");
        assert_eq!(detail_metric_rows(&statistics)[5][0], "Live objects");
    }

    #[test]
    fn gc_document_exposes_dry_run_quota_and_space_preflight() {
        let plan = ProjectCacheMaintenancePlan {
            cache_root: "generated/.blobray-cache".into(),
            present: true,
            supported: true,
            filesystem: "local".to_owned(),
            filesystem_magic: Some("0xef53".to_owned()),
            root_bytes: 900,
            reclaimable_bytes: 400,
            projected_root_bytes: 500,
            temporary_bytes_required: 300,
            available_bytes: Some(1_000),
            enough_free_space: true,
            max_size_bytes: Some(600),
            over_max_size_bytes: 0,
            would_compact: true,
            ready_to_compact: true,
            reason: None,
        };

        let document = serde_json::to_value(CacheGcDocument {
            schema_version: 1,
            command: "project cache gc",
            dry_run: true,
            plan: &plan,
        })
        .unwrap();

        assert_eq!(document["command"], "project cache gc");
        assert_eq!(document["dry_run"], true);
        assert_eq!(document["projected_root_bytes"], 500);
        assert_eq!(document["max_size_bytes"], 600);
        assert_eq!(document["ready_to_compact"], true);
    }

    #[test]
    fn retention_document_separates_obsolete_candidates_from_hard_roots() {
        let plan = ProjectCacheRetentionPlan {
            maintenance: ProjectCacheMaintenancePlan {
                cache_root: "generated/.blobray-cache".into(),
                present: true,
                supported: true,
                filesystem: "local".to_owned(),
                filesystem_magic: Some("0xef53".to_owned()),
                root_bytes: 900,
                reclaimable_bytes: 300,
                projected_root_bytes: 600,
                temporary_bytes_required: 500,
                available_bytes: Some(1_000),
                enough_free_space: true,
                max_size_bytes: Some(700),
                over_max_size_bytes: 0,
                would_compact: true,
                ready_to_compact: true,
                reason: None,
            },
            retention_seconds: 30 * 24 * 60 * 60,
            cutoff_unix_seconds: 1_700_000_000,
            retired_objects: 5,
            eligible_objects: 3,
            eligible_payload_bytes: 240,
            eligible_record_bytes: 384,
            would_prune: true,
            ready_to_prune: true,
            reason: None,
        };
        let document = serde_json::to_value(CacheRetentionPlanDocument {
            schema_version: 1,
            command: "project cache gc",
            dry_run: true,
            operation: "retention-prune",
            plan: &plan,
        })
        .unwrap();

        assert_eq!(document["dry_run"], true);
        assert_eq!(document["operation"], "retention-prune");
        assert_eq!(document["plan"]["eligible_objects"], 3);
        assert_eq!(document["plan"]["maintenance"]["max_size_bytes"], 700);
    }
}
