//! Terminal renderers for typed domain reports.

use std::fmt::Write as _;

use crate::{FunctionAnalysis, verification::*};

mod execution;

pub(crate) use execution::print_execution_comparison;

pub(crate) fn trace(trace: &FunctionAnalysis) {
    let mut output = String::new();
    let _ = writeln!(
        &mut output,
        "{}\nFunction: {}\nExact: {}",
        crate::cli::output::heading("Execution trace"),
        trace.symbol,
        trace.is_exact()
    );
    let _ = writeln!(&mut output, "\nEvents");
    for (index, event) in trace.events.iter().enumerate() {
        let _ = writeln!(&mut output, "  {index:>4}. {}", event.canonical());
    }
    if !trace.blockers.is_empty() {
        let _ = writeln!(&mut output, "\nBlockers");
        for (index, blocker) in trace.blockers.iter().enumerate() {
            let _ = writeln!(&mut output, "  {}. {blocker}", index + 1);
        }
    }
    crate::cli::output::text(output);
}

pub(crate) fn verification_human(report: &VerificationCommandReport) {
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
    for source in &report.sources {
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
    if let Some(comparison) = &report.evidence_comparison {
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
    let verdict = if comparison.passed {
        crate::cli::output::success("PASS")
    } else {
        crate::cli::output::failure("FAIL")
    };
    let _ = writeln!(
        &mut output,
        "{}\n{verdict} — {} expected, {} actual",
        crate::cli::output::heading("Evidence baseline"),
        comparison.expected,
        comparison.actual
    );
    if !comparison.regressions.is_empty() {
        let _ = writeln!(&mut output, "\nRegressions");
    }
    for regression in &comparison.regressions {
        let _ = writeln!(
            &mut output,
            "  {}/{}: expected {}, actual {}",
            regression.source,
            regression.symbol,
            regression.expected,
            regression
                .actual
                .as_ref()
                .map_or_else(|| "missing".to_owned(), ToString::to_string)
        );
        for component in &regression.changed_components {
            let _ = writeln!(
                &mut output,
                "    {}: expected {}, actual {}",
                component.name,
                component.expected.as_deref().unwrap_or("missing"),
                component.actual.as_deref().unwrap_or("missing"),
            );
        }
    }
    if !comparison.additions.is_empty() {
        let _ = writeln!(&mut output, "\nAdditions");
    }
    for addition in &comparison.additions {
        let _ = writeln!(
            &mut output,
            "  {}/{}: {}",
            addition.source, addition.symbol, addition.identity
        );
    }
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
    let _ = writeln!(&mut output, "{} coverage", side);
    for (site, taken) in required {
        let location = image.location(*site);
        if covered.contains(&(*site, *taken)) {
            if crate::cli::output::details() {
                let _ = writeln!(&mut output, "  Covered branch: {location}, taken={taken}");
            }
        } else {
            let _ = writeln!(&mut output, "  Uncovered branch: {location}, taken={taken}");
            uncovered += 1;
        }
    }
    let sites = required
        .iter()
        .map(|(site, _)| *site)
        .collect::<std::collections::BTreeSet<_>>();
    let _ = writeln!(
        &mut output,
        "  Summary: {} site(s), {} outcome(s), {} covered, {uncovered} uncovered",
        sites.len(),
        required.len(),
        required.len() - uncovered,
    );
    crate::cli::output::text(output);
    uncovered
}
