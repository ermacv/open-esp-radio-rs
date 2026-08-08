//! Terminal renderers for typed domain reports.

use crate::{FunctionAnalysis, verification::*};

mod execution;

pub(crate) use execution::print_execution_comparison;

pub(crate) fn trace(trace: &FunctionAnalysis) {
    outputln!("TRACE\t{}\texact={}", trace.symbol, trace.is_exact());
    for (index, event) in trace.events.iter().enumerate() {
        outputln!("{index}\t{}", event.canonical());
    }
    for blocker in &trace.blockers {
        outputln!("BLOCKER\t{blocker}");
    }
}

pub(crate) fn verification_human(report: &VerificationCommandReport<'_>) {
    outputln!(
        "{}: {}",
        report.command,
        if report.verification.passed {
            "passed"
        } else {
            "failed"
        }
    );
    for source in report.sources {
        outputln!(
            "  {}: {} functions, {} matched, {} mismatched, {} incomplete, {} missing",
            source.source,
            source.summary.vendor_functions,
            source.summary.matched,
            source.summary.mismatched,
            source.summary.incomplete,
            source.summary.missing,
        );
        for function in &source.functions {
            outputln!(
                "    {}: {}",
                function.vendor_symbol,
                function.status.label()
            );
        }
    }
    if let Some(comparison) = report.evidence_comparison {
        outputln!(
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
        outputln!("  report: {} ({})", publication.path, publication.status);
    }
}

pub(crate) fn verification_tsv(report: &VerificationCommandReport<'_>) {
    outputln!(
        "verification\t{}\t{}",
        report.command,
        if report.verification.passed {
            "passed"
        } else {
            "failed"
        }
    );
    for source in report.sources {
        outputln!(
            "source\t{}\tfunctions={}\tmatched={}\tmismatched={}\tincomplete={}\tmissing={}",
            source.source,
            source.summary.vendor_functions,
            source.summary.matched,
            source.summary.mismatched,
            source.summary.incomplete,
            source.summary.missing,
        );
        for function in &source.functions {
            outputln!(
                "function\t{}\t{}\t{}",
                function.source,
                function.vendor_symbol,
                function.status.label()
            );
        }
    }
}

pub(crate) fn evidence_comparison(comparison: &EvidenceComparison) {
    for regression in &comparison.regressions {
        outputln!(
            "EVIDENCE-REGRESSION\t{}\t{}\texpected={}\tactual={}",
            regression.source,
            regression.symbol,
            regression.expected,
            regression.actual.as_deref().unwrap_or("missing")
        );
    }
    for addition in &comparison.additions {
        outputln!(
            "EVIDENCE-ADDITION\t{}\t{}\t{}",
            addition.source,
            addition.symbol,
            addition.kind
        );
    }
    outputln!(
        "EVIDENCE-BASELINE\t{}\texpected={}\tactual={}",
        if comparison.passed { "PASS" } else { "FAIL" },
        comparison.expected,
        comparison.actual
    );
}

pub(crate) fn branch_coverage(
    side: &str,
    image: &crate::execution::ExecutableImage,
    required: &std::collections::BTreeSet<(u32, bool)>,
    covered: &std::collections::BTreeSet<(u32, bool)>,
) -> usize {
    let mut uncovered = 0;
    for (site, taken) in required {
        let location = image.location(*site);
        if covered.contains(&(*site, *taken)) {
            outputln!("COVERED-BRANCH\t{side}\t{location}\ttaken={taken}");
        } else {
            outputln!("UNCOVERED-BRANCH\t{side}\t{location}\ttaken={taken}");
            uncovered += 1;
        }
    }
    let sites = required
        .iter()
        .map(|(site, _)| *site)
        .collect::<std::collections::BTreeSet<_>>();
    outputln!(
        "SUMMARY-BRANCHES\t{side}\tsites={}\toutcomes={}\tcovered={}\tuncovered={uncovered}",
        sites.len(),
        required.len(),
        required.len() - uncovered,
    );
    uncovered
}
