use std::{fmt::Write as _, path::Path};

use super::{CodeBoundaryStatus, CodeWorkspace};
use crate::Result;

pub(crate) fn render_code_boundary_review(
    workspace: &CodeWorkspace,
    symbol_inventory: &Path,
) -> Result<String> {
    let summary = workspace.summary();
    let mut output = String::new();
    output.push_str("# Reviewed code boundaries\n\n");
    writeln!(
        output,
        "Generated facts: `{}`\n",
        symbol_inventory.display()
    )
    .unwrap();
    writeln!(
        output,
        "Candidates: {} · accepted: {} · rejected: {} · unreviewed: {}\n",
        summary.observed_candidates, summary.accepted, summary.rejected, summary.unreviewed
    )
    .unwrap();
    for entry in workspace.entries() {
        let status = match entry.review.status {
            CodeBoundaryStatus::Accepted => "accepted",
            CodeBoundaryStatus::Rejected => "rejected",
            CodeBoundaryStatus::Unreviewed => "unreviewed",
        };
        let address = entry.fact.section_address + entry.review.entry_offset;
        writeln!(
            output,
            "## `{}` at `{address:#x}`\n",
            entry.review.name.as_deref().unwrap_or(status)
        )
        .unwrap();
        writeln!(output, "- Status: `{status}`").unwrap();
        writeln!(output, "- Source: `{}`", entry.review.source).unwrap();
        writeln!(
            output,
            "- Artifact SHA-256: `{}`",
            entry.review.artifact_sha256
        )
        .unwrap();
        if let Some(member) = &entry.review.member {
            writeln!(output, "- Archive member: `{member}`").unwrap();
        }
        writeln!(output, "- Section: `{}`", entry.review.section).unwrap();
        writeln!(
            output,
            "- Reviewed range: `{:#x}..{:#x}`",
            entry.review.entry_offset, entry.review.end_exclusive_offset
        )
        .unwrap();
        writeln!(output, "- Object kind: `{}`", entry.fact.object_kind).unwrap();
        if !entry.fact.symbol_names.is_empty() {
            writeln!(
                output,
                "- Zero-sized symbol evidence: {}",
                entry.fact.symbol_names.join(", ")
            )
            .unwrap();
        }
        for edge in &entry.fact.direct_control_flow {
            writeln!(
                output,
                "- {} from `{}` at section offset `{:#x}`",
                edge.kind, edge.caller, edge.site_offset
            )
            .unwrap();
        }
        if let Some(reason) = &entry.review.reason {
            writeln!(output, "- Review reason: {reason}").unwrap();
        }
        output.push('\n');
    }
    Ok(output)
}
