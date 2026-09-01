//! Cross-revision function correspondence without trusting vendor names.

use super::super::*;
use crate::symbol_correspondence::{self, SymbolCorrespondenceRequest};

pub(super) fn run(arguments: SymbolCorrelateArgs) -> Result<bool> {
    let report = symbol_correspondence::correlate(SymbolCorrespondenceRequest {
        from_source: arguments.from.source.as_str(),
        from_path: &arguments.from.path,
        from_prefix: &arguments.from_prefix,
        to_source: arguments.to.source.as_str(),
        to_path: &arguments.to.path,
        to_prefix: &arguments.to_prefix,
    })?;
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
            "symbol correspondence",
            false,
        )?;
    }
    if !crate::cli::output::structured(&report) {
        outputln!("Symbol correspondence");
        outputln!(
            "From: {}@{} ({})",
            report.from.source,
            report.from.sha256,
            report.from.path
        );
        outputln!(
            "To:   {}@{} ({})",
            report.to.source,
            report.to.sha256,
            report.to.path
        );
        outputln!("Method: {}", report.method);
        outputln!(
            "Summary: source-functions={} target-functions={} unique={} graph-refined={} ambiguous={} unmatched={}",
            report.from.functions,
            report.to.functions,
            report.summary.unique,
            report.summary.graph_refined,
            report.summary.ambiguous,
            report.summary.unmatched,
        );
        if crate::cli::output::details() {
            let rows = report
                .correspondences
                .iter()
                .filter(|correspondence| !correspondence.candidates.is_empty())
                .map(|correspondence| {
                    [
                        correspondence
                            .from
                            .member
                            .as_deref()
                            .unwrap_or("-")
                            .to_owned(),
                        correspondence.from.symbol.clone(),
                        format!("{:?}", correspondence.status).to_ascii_lowercase(),
                        correspondence.basis.to_owned(),
                        correspondence
                            .candidates
                            .iter()
                            .map(|candidate| {
                                candidate.member.as_deref().map_or_else(
                                    || candidate.symbol.clone(),
                                    |member| format!("{member}:{}", candidate.symbol),
                                )
                            })
                            .collect::<Vec<_>>()
                            .join(", "),
                    ]
                });
            outputln!(
                "Matches:\n{}",
                crate::cli::table::render(
                    ["Member", "Old symbol", "Status", "Basis", "New candidate"],
                    rows
                )
            );
        }
        if let Some(publication) = publication {
            outputln!("Publication: {} — {}", publication.status, publication.path);
        }
    }
    Ok(true)
}
