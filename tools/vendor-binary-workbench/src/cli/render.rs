//! Terminal renderers for typed domain reports.

use std::fmt::Write as _;

use crate::{FunctionAnalysis, verification::*};

mod execution;

pub(crate) use execution::print_execution_comparison;

pub(crate) fn trace(trace: &FunctionAnalysis) {
    let mut output = String::new();
    let _ = writeln!(
        &mut output,
        "TRACE\t{}\texact={}",
        trace.symbol,
        trace.is_exact()
    );
    for (index, event) in trace.events.iter().enumerate() {
        let _ = writeln!(&mut output, "{index}\t{}", event.canonical());
    }
    for blocker in &trace.blockers {
        let _ = writeln!(&mut output, "BLOCKER\t{blocker}");
    }
    crate::cli::output::text(output);
}

pub(crate) fn verification_human(report: &VerificationCommandReport<'_>) {
    let mut output = String::new();
    let _ = writeln!(
        &mut output,
        "{}: {}",
        report.command,
        if report.verification.passed {
            "passed"
        } else {
            "failed"
        }
    );
    for source in report.sources {
        let _ = writeln!(
            &mut output,
            "  {}: {} functions, {} matched, {} mismatched, {} incomplete, {} missing",
            source.source,
            source.summary.vendor_functions,
            source.summary.matched,
            source.summary.mismatched,
            source.summary.incomplete,
            source.summary.missing,
        );
        for function in &source.functions {
            let _ = writeln!(
                &mut output,
                "    {}: {}",
                function.vendor_symbol,
                function.status.label()
            );
        }
    }
    if let Some(comparison) = report.evidence_comparison {
        let _ = writeln!(
            &mut output,
            "  evidence baseline: {} ({} expected, {} actual, {} regressions)",
            if comparison.passed {
                "passed"
            } else {
                "failed"
            },
            comparison.expected,
            comparison.actual,
            comparison.regressions.len(),
        );
    }
    if let Some(publication) = &report.report {
        let _ = writeln!(
            &mut output,
            "  report: {} ({})",
            publication.path, publication.status
        );
    }
    crate::cli::output::text(output);
}

pub(crate) fn evidence_comparison(comparison: &EvidenceComparison) {
    let mut output = String::new();
    for regression in &comparison.regressions {
        let _ = writeln!(
            &mut output,
            "EVIDENCE-REGRESSION\t{}\t{}\texpected={}\tactual={}",
            regression.source,
            regression.symbol,
            regression.expected,
            regression.actual.as_deref().unwrap_or("missing")
        );
    }
    for addition in &comparison.additions {
        let _ = writeln!(
            &mut output,
            "EVIDENCE-ADDITION\t{}\t{}\t{}",
            addition.source, addition.symbol, addition.kind
        );
    }
    let _ = writeln!(
        &mut output,
        "EVIDENCE-BASELINE\t{}\texpected={}\tactual={}",
        if comparison.passed { "PASS" } else { "FAIL" },
        comparison.expected,
        comparison.actual
    );
    crate::cli::output::text(output);
}

pub(crate) fn branch_coverage(
    side: &str,
    image: &crate::execution::ExecutableImage,
    required: &std::collections::BTreeSet<(u32, bool)>,
    covered: &std::collections::BTreeSet<(u32, bool)>,
) -> usize {
    let mut output = String::new();
    let mut uncovered = 0;
    for (site, taken) in required {
        let location = image.location(*site);
        if covered.contains(&(*site, *taken)) {
            let _ = writeln!(
                &mut output,
                "COVERED-BRANCH\t{side}\t{location}\ttaken={taken}"
            );
        } else {
            let _ = writeln!(
                &mut output,
                "UNCOVERED-BRANCH\t{side}\t{location}\ttaken={taken}"
            );
            uncovered += 1;
        }
    }
    let sites = required
        .iter()
        .map(|(site, _)| *site)
        .collect::<std::collections::BTreeSet<_>>();
    let _ = writeln!(
        &mut output,
        "SUMMARY-BRANCHES\t{side}\tsites={}\toutcomes={}\tcovered={}\tuncovered={uncovered}",
        sites.len(),
        required.len(),
        required.len() - uncovered,
    );
    crate::cli::output::text(output);
    uncovered
}
