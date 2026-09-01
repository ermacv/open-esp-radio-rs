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
            "Obfuscation epoch: {:?}; functions={:?} (source={} target={} common={} retained={:.3}%) data={:?} (source={} target={} common={} retained={:.3}%)",
            report.obfuscation_epoch.status,
            report.obfuscation_epoch.functions.status,
            report.obfuscation_epoch.functions.from_tokens,
            report.obfuscation_epoch.functions.to_tokens,
            report.obfuscation_epoch.functions.common_tokens,
            f64::from(
                report
                    .obfuscation_epoch
                    .functions
                    .smaller_set_retention_parts_per_million
            ) / 10_000.0,
            report.obfuscation_epoch.data_objects.status,
            report.obfuscation_epoch.data_objects.from_tokens,
            report.obfuscation_epoch.data_objects.to_tokens,
            report.obfuscation_epoch.data_objects.common_tokens,
            f64::from(
                report
                    .obfuscation_epoch
                    .data_objects
                    .smaller_set_retention_parts_per_million
            ) / 10_000.0,
        );
        outputln!(
            "Summary: source-functions={} target-functions={} unique={} name-stable={} token-stable={} graph-refined={} ambiguous={} unmatched={}",
            report.from.functions,
            report.to.functions,
            report.summary.unique,
            report.summary.name_stable,
            report.summary.token_stable,
            report.summary.graph_refined,
            report.summary.ambiguous,
            report.summary.unmatched,
        );
        if let Some(member_order) = &report.member_order {
            outputln!(
                "Member order: {} mappings, exact-body support={} conflicts={} support={:.3}% data-refinement={}",
                member_order.correspondences.len(),
                member_order.exact_function_support,
                member_order.exact_function_conflicts,
                f64::from(member_order.support_parts_per_million) / 10_000.0,
                if member_order.automatic_data_matches {
                    "enabled"
                } else {
                    "disabled"
                },
            );
        }
        outputln!(
            "Data objects: source={} target={} unique={} name-stable={} token-stable={} reference-refined={} member-refined={} ambiguous={} unmatched={}",
            report.data_summary.from_objects,
            report.data_summary.to_objects,
            report.data_summary.unique,
            report.data_summary.name_stable,
            report.data_summary.token_stable,
            report.data_summary.reference_refined,
            report.data_summary.member_refined,
            report.data_summary.ambiguous,
            report.data_summary.unmatched,
        );
        outputln!(
            "Pin candidates: {} (all require review)",
            report.pin_candidates.len()
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
