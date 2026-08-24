//! Read-only inspection of the project-owned incremental query cache.

use std::path::Path;

use crate::{
    Result,
    application::{ProjectCacheStatistics, project_cache_statistics},
    cli::{output, table},
};

#[derive(serde::Serialize)]
struct CacheStatsDocument<'a> {
    schema_version: u32,
    command: &'static str,
    #[serde(flatten)]
    statistics: &'a ProjectCacheStatistics,
}

pub(super) fn stats(project_manifest: &Path) -> Result<bool> {
    let statistics = project_cache_statistics(project_manifest)?;
    let document = CacheStatsDocument {
        schema_version: 1,
        command: "project cache stats",
        statistics: &statistics,
    };
    output::render_report(&document, || render_human(&statistics));
    Ok(true)
}

fn render_human(statistics: &ProjectCacheStatistics) {
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
        let statistics = ProjectCacheStatistics {
            present: false,
            cache_root: "generated/.blobray-cache".into(),
            database_path: "generated/.blobray-cache/queries.sqlite3".into(),
            schema: None,
            root_bytes: 0,
            database_bytes: 0,
            pack_bytes: 0,
            query_results: 0,
            inline_bytes: 0,
            dependencies: 0,
            objects: 0,
            object_payload_bytes: 0,
            stage_bindings: 0,
            stage_outputs: 0,
            live_objects: 0,
            live_record_bytes: 0,
            reclaimable_pack_bytes: 0,
            query_kinds: Vec::new(),
        };
        let document = serde_json::to_value(CacheStatsDocument {
            schema_version: 1,
            command: "project cache stats",
            statistics: &statistics,
        })
        .unwrap();
        assert_eq!(document["schema_version"], 1);
        assert_eq!(document["command"], "project cache stats");
        assert_eq!(document["schema"], serde_json::Value::Null);
        assert_eq!(document["present"], false);
        assert_eq!(detail_metric_rows(&statistics).len(), 7);
        assert_eq!(detail_metric_rows(&statistics)[0][0], "Store schema");
        assert_eq!(detail_metric_rows(&statistics)[5][0], "Live objects");
    }
}
