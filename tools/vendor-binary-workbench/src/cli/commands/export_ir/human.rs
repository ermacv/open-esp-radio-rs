//! Compact human-readable linked-IR report rendering.
//!
//! The linked IR itself can be several megabytes. Human stdout is a status
//! surface, not a second serialization of the complete report: detailed
//! evidence belongs in `--json-report` and generated pseudo code belongs in
//! `--pseudo-rust`.

use super::*;
use std::fmt::Write as _;

const MAX_FUNCTION_ROWS: usize = 64;

pub(super) fn print_report(
    artifacts: &[IrArtifactInput],
    report: &LinkedIrReport,
    include_reachable: bool,
) {
    let mut output = String::new();
    let root_functions = report
        .functions
        .iter()
        .filter(|function| function.selection == "symbol-prefix-root")
        .count();
    let diagnostics = report
        .functions
        .iter()
        .map(|function| {
            function.call_graph_diagnostics.len()
                + function.direct_diagnostics.len()
                + function.reference_diagnostics.len()
        })
        .sum::<usize>();
    let mut representative_functions = report.functions.iter().collect::<Vec<_>>();
    representative_functions.sort_by(|left, right| {
        function_priority(right)
            .cmp(&function_priority(left))
            .then_with(|| left.identity.cmp(&right.identity))
    });
    representative_functions.truncate(MAX_FUNCTION_ROWS);
    representative_functions.sort_by(|left, right| left.identity.cmp(&right.identity));
    let omitted_functions = report
        .functions
        .len()
        .saturating_sub(representative_functions.len());

    let _ = writeln!(output, "Linked IR");
    let _ = writeln!(
        output,
        "Artifacts:\n{}",
        crate::cli::table::render(
            ["Source", "Reviewed ranges", "Path"],
            artifacts.iter().map(|artifact| [
                artifact.source.clone(),
                artifact.reviewed_code.len().to_string(),
                crate::cli::table::compact(artifact.path.display().to_string(), 72),
            ]),
        )
    );
    let _ = writeln!(
        output,
        "Functions ({} most active of {}, {} omitted):\n{}",
        representative_functions.len(),
        report.functions.len(),
        omitted_functions,
        crate::cli::table::render(
            [
                "Function",
                "Flow",
                "Exact",
                "MMIO",
                "Calls",
                "Memory",
                "Diagnostics",
            ],
            representative_functions.into_iter().map(|function| [
                crate::cli::table::compact(&function.identity, 56),
                function.flow_kind.to_owned(),
                if function.exact { "yes" } else { "no" }.to_owned(),
                function.mmio_accesses.len().to_string(),
                function.calls.len().to_string(),
                function.memory_accesses.len().to_string(),
                (function.call_graph_diagnostics.len()
                    + function.direct_diagnostics.len()
                    + function.reference_diagnostics.len())
                .to_string(),
            ]),
        )
    );
    let _ = writeln!(
        output,
        "Summary: artifacts={} functions={} roots={} reachable={} decode-blockers={} MMIO-registers={} MMIO-shapes={} field-candidates={} semantic-calls={} complete={} diagnostics={}",
        artifacts.len(),
        report.functions.len(),
        root_functions,
        if include_reachable {
            report.functions.len().saturating_sub(root_functions)
        } else {
            0
        },
        report
            .functions
            .iter()
            .map(|function| function.decode_blockers.len())
            .sum::<usize>(),
        report.mmio_registers.len(),
        report.mmio_access_shapes,
        report
            .mmio_registers
            .iter()
            .map(|register| register.field_candidates.len())
            .sum::<usize>(),
        report.semantic_calls,
        report.complete_functions,
        diagnostics,
    );
    let _ = writeln!(
        output,
        "Detailed effects, register fields, provenance and blockers are available in the JSON report and pseudo-Rust output."
    );
    crate::cli::output::text(output);
}

fn function_priority(function: &LinkedIrFunction) -> (bool, usize, usize, usize, usize, usize) {
    let diagnostics = function.call_graph_diagnostics.len()
        + function.direct_diagnostics.len()
        + function.reference_diagnostics.len();
    (
        function.effect_summary.semantic_action_count != 0,
        function.mmio_accesses.len(),
        function.calls.len(),
        function.memory_accesses.len(),
        function.context_accesses.len(),
        diagnostics,
    )
}
