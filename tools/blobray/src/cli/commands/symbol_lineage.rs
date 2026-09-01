//! Multi-revision symbol-identity lineage.

use super::super::*;
use crate::symbol_lineage::{
    self, SymbolLineageFrontierRoute, SymbolLineageRevision, SymbolLineageStatus,
};

pub(super) fn run(arguments: SymbolLineageArgs) -> Result<bool> {
    let revisions = arguments
        .revisions
        .iter()
        .map(|revision| SymbolLineageRevision {
            label: &revision.label,
            source: arguments.source.as_str(),
            path: &revision.path,
        })
        .collect::<Vec<_>>();
    let report = symbol_lineage::build(&revisions)?;
    let publication = arguments.output.as_deref().map(|path| {
        crate::cli::output::Publication::new(
            path,
            if arguments.check {
                "verified"
            } else {
                "written"
            },
        )
    });
    if let Some(path) = arguments.output.as_deref() {
        crate::application::generated_file::write_or_check_json(
            path,
            &report,
            arguments.check,
            "symbol lineage",
            false,
        )?;
    }
    if !crate::cli::output::structured(&report) {
        outputln!("Symbol lineage");
        outputln!("Method: {}", report.method);
        outputln!("Artifacts: {}", report.artifacts.len());
        outputln!(
            "Functions: source={} resolved={} confirmed={} direct-only={} chain-only={} conflicts={} unresolved={}",
            report.function_summary.source,
            report.function_summary.resolved,
            report.function_summary.confirmed,
            report.function_summary.direct_only,
            report.function_summary.chain_only,
            report.function_summary.conflict,
            report.function_summary.unresolved,
        );
        outputln!(
            "Data objects: source={} resolved={} confirmed={} direct-only={} chain-only={} conflicts={} unresolved={}",
            report.data_summary.source,
            report.data_summary.resolved,
            report.data_summary.confirmed,
            report.data_summary.direct_only,
            report.data_summary.chain_only,
            report.data_summary.conflict,
            report.data_summary.unresolved,
        );
        outputln!(
            "Pin candidates: {} (all require review)",
            report.pin_candidates.len()
        );
        if crate::cli::output::details() {
            let rows = report
                .edges
                .iter()
                .map(|edge| {
                    [
                        edge.index
                            .expect("ordered lineage edges always have an index")
                            .to_string(),
                        edge.from_label.clone(),
                        edge.to_label.clone(),
                        format!("{:?}", edge.obfuscation_epoch.status).to_ascii_lowercase(),
                        edge.functions.unique.to_string(),
                        edge.data_objects.unique.to_string(),
                    ]
                })
                .chain(std::iter::once([
                    "direct".to_owned(),
                    report.direct.from_label.clone(),
                    report.direct.to_label.clone(),
                    format!("{:?}", report.direct.obfuscation_epoch.status).to_ascii_lowercase(),
                    report.direct.functions.unique.to_string(),
                    report.direct.data_objects.unique.to_string(),
                ]));
            outputln!(
                "{}",
                crate::cli::table::render(
                    ["Edge", "From", "To", "Epoch", "Functions", "Data objects"],
                    rows,
                )
            );
            if !report.review_frontiers.is_empty() {
                outputln!(
                    "\nReview frontiers (reviewable facts / total records; highest impact first)"
                );
                outputln!(
                    "{}",
                    crate::cli::table::render(
                        ["Facts", "Domain", "Affected", "Frontier", "Evidence"],
                        report.review_frontiers.iter().map(|frontier| {
                            let candidates = if frontier.candidate_min == frontier.candidate_max {
                                frontier.candidate_min.to_string()
                            } else {
                                format!("{}..{}", frontier.candidate_min, frontier.candidate_max)
                            };
                            let frontier_name = frontier.edge.map_or_else(
                                || {
                                    format!(
                                        "{}: {} → {}",
                                        frontier_route_label(frontier.route),
                                        frontier.from,
                                        frontier.to
                                    )
                                },
                                |edge| format!("edge {edge}: {} → {}", frontier.from, frontier.to),
                            );
                            [
                                if frontier.reviewable_records == frontier.records {
                                    frontier.records.to_string()
                                } else {
                                    format!("{}/{}", frontier.reviewable_records, frontier.records)
                                },
                                frontier.domain.to_owned(),
                                lineage_status_label(frontier.affected_status).to_owned(),
                                frontier_name,
                                format!(
                                    "{}; {}; candidates={candidates}",
                                    frontier
                                        .correspondence_status
                                        .map(|status| format!("{status:?}").to_ascii_lowercase())
                                        .unwrap_or_else(|| "conflict".to_owned()),
                                    frontier.basis,
                                ),
                            ]
                        }),
                    )
                );
            }
        }
        if let Some(publication) = publication {
            outputln!("Publication: {} — {}", publication.status, publication.path);
        }
    }
    Ok(true)
}

fn lineage_status_label(status: SymbolLineageStatus) -> &'static str {
    match status {
        SymbolLineageStatus::Confirmed => "confirmed",
        SymbolLineageStatus::DirectOnly => "direct-only",
        SymbolLineageStatus::ChainOnly => "chain-only",
        SymbolLineageStatus::Conflict => "conflict",
        SymbolLineageStatus::Unresolved => "unresolved",
    }
}

fn frontier_route_label(route: SymbolLineageFrontierRoute) -> &'static str {
    match route {
        SymbolLineageFrontierRoute::AdjacentChain => "adjacent-chain",
        SymbolLineageFrontierRoute::DirectEndpoint => "direct-endpoint",
        SymbolLineageFrontierRoute::EndpointConflict => "endpoint-conflict",
    }
}
