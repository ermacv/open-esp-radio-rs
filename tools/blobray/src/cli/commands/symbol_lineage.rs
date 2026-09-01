//! Multi-revision symbol-identity lineage.

use super::super::*;
use crate::symbol_lineage::{self, SymbolLineageRevision};

pub(super) fn run(arguments: SymbolLineageArgs) -> Result<bool> {
    let revisions = arguments
        .revisions
        .iter()
        .map(|revision| SymbolLineageRevision {
            source: revision.source.as_str(),
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
            let rows = report.edges.iter().map(|edge| {
                [
                    edge.index
                        .expect("ordered lineage edges always have an index")
                        .to_string(),
                    edge.from.source.clone(),
                    edge.to.source.clone(),
                    format!("{:?}", edge.obfuscation_epoch.status).to_ascii_lowercase(),
                    edge.functions.unique.to_string(),
                    edge.data_objects.unique.to_string(),
                ]
            });
            outputln!(
                "{}",
                crate::cli::table::render(
                    ["Edge", "From", "To", "Epoch", "Functions", "Data objects"],
                    rows,
                )
            );
        }
        if let Some(publication) = publication {
            outputln!("Publication: {} — {}", publication.status, publication.path);
        }
    }
    Ok(true)
}
