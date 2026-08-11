//! Task-first terminal renderer for concrete execution comparisons.

use std::fmt::Write as _;

use crate::{
    cli::{output as command_output, table},
    verification::*,
};

fn trace_item_text(item: Option<&TraceItemReport>) -> String {
    let Some(item) = item else {
        return "<missing>".to_owned();
    };
    match item {
        TraceItemReport::Event { event, producer } => {
            let producer = producer.as_ref().map_or_else(String::new, |producer| {
                format!(
                    " [{}+{:#x} @ {:#010x}]",
                    producer.symbol.as_deref().unwrap_or("unknown"),
                    producer.symbol_offset.unwrap_or_default(),
                    producer.pc
                )
            });
            let event = match event {
                ExecutionEventReport::Read {
                    width,
                    address,
                    region,
                    register,
                    value,
                } => format!(
                    "READ/{width} {address:#010x} {region}/{} -> {value:#010x}",
                    register.as_deref().unwrap_or("unknown")
                ),
                ExecutionEventReport::Write {
                    width,
                    address,
                    region,
                    register,
                    value,
                } => format!(
                    "WRITE/{width} {address:#010x} {region}/{} <- {value:#010x}",
                    register.as_deref().unwrap_or("unknown")
                ),
                ExecutionEventReport::DelayMicros { micros } => format!("DELAY {micros} us"),
                ExecutionEventReport::Fence {
                    fm,
                    predecessor,
                    successor,
                } => format!("FENCE fm={fm:#x} pred={predecessor:#x} succ={successor:#x}"),
            };
            format!("{event}{producer}")
        }
        TraceItemReport::Memory { change } => format!(
            "RAM {:#010x}: {:#04x} -> {:#04x}",
            change.address, change.before, change.after
        ),
        TraceItemReport::ReturnValue { value } => format!("RETURN {value:#010x}"),
        TraceItemReport::Coverage { issue } => issue.clone(),
    }
}

fn render_difference(output: &mut String, case: &str, difference: &TraceDiffReport) {
    let _ = writeln!(
        output,
        "\nFirst difference — case {case}, {}, event #{}",
        difference.kind.label(),
        difference.first_difference
    );
    let rows = difference
        .context_before
        .iter()
        .map(|item| {
            [
                item.index.to_string(),
                trace_item_text(item.vendor.as_ref()),
                trace_item_text(item.rust.as_ref()),
            ]
        })
        .chain(std::iter::once([
            format!("{} *", difference.first_difference),
            trace_item_text(difference.vendor.as_ref()),
            trace_item_text(difference.rust.as_ref()),
        ]))
        .chain(difference.context_after.iter().map(|item| {
            [
                item.index.to_string(),
                trace_item_text(item.vendor.as_ref()),
                trace_item_text(item.rust.as_ref()),
            ]
        }));
    let _ = writeln!(
        output,
        "{}",
        table::render(["Event", "Vendor", "Rust"], rows)
    );
    if let Some(path) = &difference.path {
        let _ = writeln!(
            output,
            "Path context: vendor {} branch(es)/{} call(s), Rust {} branch(es)/{} call(s)",
            path.vendor.branches.len(),
            path.vendor.calls.len(),
            path.rust.branches.len(),
            path.rust.calls.len()
        );
    }
}

fn render_coverage(output: &mut String, side: &str, coverage: &CoverageReport) {
    let sites = coverage
        .branch_outcomes
        .iter()
        .map(|outcome| outcome.site)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let uncovered = coverage.uncovered_branch_outcomes();
    let unresolved = coverage
        .unresolved_control_flow
        .iter()
        .filter(|edge| !edge.covered)
        .count();
    let _ = writeln!(
        output,
        "{side}: {} branch site(s), {}/{} outcome(s) covered, {} unresolved edge(s), {} unnamed MMIO address(es)",
        sites,
        coverage.branch_outcomes.len() - uncovered,
        coverage.branch_outcomes.len(),
        unresolved,
        coverage.unnamed_mmio.len()
    );
    for outcome in coverage
        .branch_outcomes
        .iter()
        .filter(|outcome| !outcome.covered)
    {
        let _ = writeln!(
            output,
            "  Uncovered branch: {} taken={}",
            outcome.location, outcome.taken
        );
    }
    for edge in coverage
        .unresolved_control_flow
        .iter()
        .filter(|edge| !edge.covered)
    {
        let _ = writeln!(
            output,
            "  Unresolved control flow: {} ({})",
            edge.location, edge.edge
        );
    }
    for address in &coverage.unnamed_mmio {
        let _ = writeln!(output, "  Unnamed MMIO: {address:#010x}");
    }
    if command_output::details() {
        for call in &coverage.covered_calls {
            let _ = writeln!(output, "  Covered call: {call}");
        }
        for outcome in coverage
            .branch_outcomes
            .iter()
            .filter(|outcome| outcome.covered)
        {
            let _ = writeln!(
                output,
                "  Covered branch: {} taken={}",
                outcome.location, outcome.taken
            );
        }
    }
}

pub(crate) fn print_execution_comparison(report: &ExecutionComparisonReport) {
    let mut output = String::new();
    let verdict = match report.verdict.label() {
        "match" | "pass" => command_output::success(report.verdict.label().to_uppercase()),
        "incomplete" => command_output::warning("INCOMPLETE"),
        _ => command_output::failure(report.verdict.label().to_uppercase()),
    };
    let _ = writeln!(
        &mut output,
        "{}",
        command_output::heading("Execution comparison")
    );
    let _ = writeln!(&mut output, "{verdict} — mode {}", report.mode.label());
    let _ = writeln!(&mut output, "Vendor: {}", report.vendor.path);
    if command_output::details() {
        let _ = writeln!(&mut output, "SHA-256: {}", report.vendor.sha256);
        if let Some(companion) = &report.vendor.companion {
            let _ = writeln!(
                &mut output,
                "Companion: {} ({})",
                companion.path, companion.sha256
            );
        }
    }

    let rows = report.cases.iter().map(|case| match case {
        CaseReport::Match {
            name,
            events,
            memory_changes,
            return_compared,
            ..
        } => [
            name.clone(),
            "match".to_owned(),
            format!(
                "{events} events, {memory_changes} RAM changes, return {}",
                if *return_compared {
                    "checked"
                } else {
                    "ignored"
                }
            ),
        ],
        CaseReport::Incomplete {
            name,
            vendor_error,
            rust_error,
            ..
        } => [
            name.clone(),
            "incomplete".to_owned(),
            format!(
                "vendor: {}; Rust: {}",
                vendor_error.as_deref().unwrap_or("complete"),
                rust_error.as_deref().unwrap_or("complete")
            ),
        ],
        CaseReport::Diff {
            name, difference, ..
        } => [
            name.clone(),
            "different".to_owned(),
            format!(
                "{} at event #{}",
                difference.kind.label(),
                difference.first_difference
            ),
        ],
    });
    let _ = writeln!(&mut output, "\n{}", command_output::heading("Cases"));
    let _ = writeln!(
        &mut output,
        "{}",
        table::render(["Case", "Result", "Details"], rows)
    );

    for case in &report.cases {
        match case {
            CaseReport::Diff {
                name, difference, ..
            } => render_difference(&mut output, name, difference),
            CaseReport::Match {
                name, environment, ..
            }
            | CaseReport::Incomplete {
                name, environment, ..
            } if command_output::details() => render_environment(&mut output, name, environment),
            _ => {}
        }
    }
    if let Some(gap) = &report.coverage_gap {
        render_difference(&mut output, "coverage", gap);
    }

    let _ = writeln!(&mut output, "\n{}", command_output::heading("Coverage"));
    render_coverage(&mut output, "Vendor", &report.vendor_coverage);
    render_coverage(&mut output, "Rust", &report.rust_coverage);
    let summary = &report.summary;
    let _ = writeln!(
        &mut output,
        "\nSummary: {} case(s), {} matched, {} different, {} incomplete",
        summary.cases, summary.matched, summary.different, summary.incomplete
    );
    crate::cli::output::text(output);
}

fn render_environment(output: &mut String, case: &str, environment: &ScenarioEnvironmentReport) {
    let tables = environment.vendor_tables.len() + environment.rust_tables.len();
    let allocations = environment.vendor_allocations.len() + environment.rust_allocations.len();
    let memory =
        environment.vendor_memory_instances.len() + environment.rust_memory_instances.len();
    let _ = writeln!(
        output,
        "\nEnvironment {case}: {tables} table instance(s), {allocations} allocation(s), {} device model(s), {memory} memory instance(s)",
        environment.device_models.len()
    );
    for device in &environment.device_models {
        let _ = writeln!(
            output,
            "  Device {}: {} at {:#010x}, length {:#x}",
            device.id, device.kind, device.start, device.length
        );
    }
    for (side, coverage) in [
        ("Vendor", &environment.vendor_device_coverage),
        ("Rust", &environment.rust_device_coverage),
    ] {
        for model in coverage {
            let _ = writeln!(
                output,
                "  {side} device {}: {}{}",
                model.id,
                if model.complete {
                    "complete"
                } else {
                    "incomplete"
                },
                model
                    .reason
                    .as_deref()
                    .map_or_else(String::new, |reason| format!(" — {reason}"))
            );
        }
    }
}
