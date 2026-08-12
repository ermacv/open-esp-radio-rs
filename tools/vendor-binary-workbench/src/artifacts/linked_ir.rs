//! Typed summary projection for stored linked-IR artifacts.

use std::path::Path;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct LinkedIrSummary {
    pub(crate) functions: usize,
    pub(crate) decode_blockers: usize,
    pub(crate) registers: usize,
    pub(crate) field_candidates: usize,
}

pub(crate) fn inspect_linked_ir(path: &Path) -> crate::Result<LinkedIrSummary> {
    let reader = super::LinkedIrReader::open(path).map_err(|error| {
        crate::Error::invalid(format!(
            "unsupported linked-IR artifact in {}: {error}",
            path.display()
        ))
    })?;
    let summary = reader.summary();
    Ok(LinkedIrSummary {
        functions: summary.functions,
        decode_blockers: summary.decode_blockers,
        registers: summary.mmio_registers,
        field_candidates: summary.mmio_field_candidates,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn document() -> serde_json::Value {
        let mut document: serde_json::Value = serde_json::from_str(
            &crate::artifacts::render_linked_ir_fixture(Vec::new(), Vec::new()),
        )
        .unwrap();
        document["summary"]["functions"] = serde_json::json!(3);
        document["summary"]["decode_blockers"] = serde_json::json!(5);
        document["summary"]["mmio_registers"] = serde_json::json!(2);
        document["summary"]["mmio_field_candidates"] = serde_json::json!(4);
        document
    }

    #[test]
    fn validates_identity_claims_and_reads_summary() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-linked-ir-artifact-{}",
            std::process::id()
        ));
        super::super::write_fixture_bundle(
            &path,
            &serde_json::to_string_pretty(&document()).unwrap(),
        )
        .unwrap();
        let summary = inspect_linked_ir(&path).unwrap();
        assert_eq!(summary.functions, 3);
        assert_eq!(summary.decode_blockers, 5);
        assert_eq!(summary.registers, 2);
        assert_eq!(summary.field_candidates, 4);

        let mut stale = document();
        stale["schema_version"] = serde_json::json!(34);
        std::fs::write(path.join("manifest.json"), stale.to_string()).unwrap();
        assert!(
            inspect_linked_ir(&path)
                .unwrap_err()
                .to_string()
                .contains("expected schema_version 52")
        );
        std::fs::remove_dir_all(path).unwrap();
    }

    #[test]
    fn rejects_unknown_and_missing_fields_at_every_projection_boundary() {
        let mut unknown = document();
        unknown["summary"]["legacy_field"] = serde_json::json!(true);
        let error = super::super::parse_linked_ir(&unknown.to_string()).unwrap_err();
        assert!(error.to_string().contains("unknown field `legacy_field`"));

        let mut missing = document();
        missing
            .as_object_mut()
            .unwrap()
            .remove("scenario_suggestion_mode");
        let error = super::super::parse_linked_ir(&missing.to_string()).unwrap_err();
        assert!(
            error
                .to_string()
                .contains("missing field `scenario_suggestion_mode`")
        );
    }

    #[test]
    fn bounded_graph_search_skips_external_calls_and_reports_limits() {
        let path = std::env::temp_dir().join(format!(
            "vendor-workbench-linked-ir-graph-{}",
            std::process::id()
        ));
        super::super::write_fixture_bundle(
            &path,
            &serde_json::to_string_pretty(&document()).unwrap(),
        )
        .unwrap();
        std::fs::write(
            path.join("graph.json"),
            serde_json::json!({
                "schema_version": super::super::LINKED_IR.version,
                "command": "ir graph index",
                "edges": [
                    {"caller": "root", "callee": "external-api", "site": 1, "kind": "external"},
                    {"caller": "root", "callee": "child", "site": 2, "kind": "internal"},
                    {"caller": "child", "callee": "external-callback", "site": 3, "kind": "external"},
                    {"caller": "child", "callee": "sink", "site": 4, "kind": "project-linked"},
                    {"caller": "sink", "callee": "leaf", "site": 5, "kind": "indexed-dispatch"}
                ]
            })
            .to_string(),
        )
        .unwrap();

        let reader = super::super::LinkedIrReader::open(&path).unwrap();
        let path_result = reader.shortest_path_to_any(
            "root",
            &std::collections::BTreeSet::from(["sink".to_owned()]),
            super::super::GraphSearchLimits {
                max_depth: 2,
                max_visited_nodes: 8,
                max_examined_edges: 16,
            },
        );
        assert_eq!(
            path_result
                .path
                .unwrap()
                .iter()
                .map(|edge| edge.callee.as_str())
                .collect::<Vec<_>>(),
            ["child", "sink"]
        );
        let indexed_path = reader.shortest_path_to_any(
            "root",
            &std::collections::BTreeSet::from(["leaf".to_owned()]),
            super::super::GraphSearchLimits {
                max_depth: 3,
                max_visited_nodes: 8,
                max_examined_edges: 16,
            },
        );
        assert_eq!(
            indexed_path.path.unwrap().last().unwrap().kind,
            "indexed-dispatch"
        );

        let depth_limited = reader.reachable_from(
            "root",
            super::super::GraphSearchLimits {
                max_depth: 1,
                max_visited_nodes: 8,
                max_examined_edges: 16,
            },
        );
        assert_eq!(
            depth_limited.identities,
            std::collections::BTreeSet::from(["root".to_owned(), "child".to_owned()])
        );
        assert_eq!(depth_limited.limit, Some("max-depth"));

        let node_limited = reader.reachable_from(
            "root",
            super::super::GraphSearchLimits {
                max_depth: 8,
                max_visited_nodes: 2,
                max_examined_edges: 16,
            },
        );
        assert_eq!(node_limited.limit, Some("max-visited-nodes"));
        assert!(!node_limited.identities.contains("external-api"));

        let slice = reader.graph_slice(
            "root",
            1,
            false,
            super::super::GraphSearchLimits {
                max_depth: 1,
                max_visited_nodes: 8,
                max_examined_edges: 16,
            },
        );
        assert_eq!(slice.visited_nodes, 2);
        assert_eq!(slice.limit, Some("max-depth"));
        assert!(slice.edges.iter().any(|edge| edge.callee == "external-api"));
        std::fs::remove_dir_all(path).unwrap();
    }
}
