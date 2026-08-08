//! Terminal renderer for concrete execution comparisons.

use std::fmt::Write as _;

use crate::verification::*;

fn trace_item_text(item: Option<&TraceItemReport>) -> String {
    let Some(item) = item else {
        return "<missing>".to_owned();
    };
    match item {
        TraceItemReport::Event { event, producer } => {
            let producer = producer.as_ref().map_or_else(String::new, |producer| {
                format!(
                    " producer={}+{:#x}@{:#010x}",
                    producer.symbol.as_deref().unwrap_or("<unknown>"),
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
                    "READ/{width} {address:#010x} region={region} register={} -> {value:#010x}",
                    register.as_deref().unwrap_or("-")
                ),
                ExecutionEventReport::Write {
                    width,
                    address,
                    region,
                    register,
                    value,
                } => format!(
                    "WRITE/{width} {address:#010x} region={region} register={} <- {value:#010x}",
                    register.as_deref().unwrap_or("-")
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
            "RAM {:#010x} {:#04x} -> {:#04x}",
            change.address, change.before, change.after
        ),
        TraceItemReport::ReturnValue { value } => format!("RETURN {value:#010x}"),
        TraceItemReport::Coverage { issue } => issue.clone(),
    }
}

fn render_difference(mut output: &mut String, case: &str, difference: &TraceDiffReport) {
    let _ = writeln!(
        &mut output,
        "FIRST-DIFFERENCE\tcase={case}\tkind={}\tindex={}",
        difference.kind.label(),
        difference.first_difference
    );
    for item in &difference.context_before {
        let _ = writeln!(
            &mut output,
            "DIFF-CONTEXT\tbefore\t{}\tequal={}\tvendor={}\trust={}",
            item.index,
            item.equal,
            trace_item_text(item.vendor.as_ref()),
            trace_item_text(item.rust.as_ref())
        );
    }
    let _ = writeln!(
        &mut output,
        "DIFF-ITEM\t{}\tvendor={}\trust={}",
        difference.first_difference,
        trace_item_text(difference.vendor.as_ref()),
        trace_item_text(difference.rust.as_ref())
    );
    for item in &difference.context_after {
        let _ = writeln!(
            &mut output,
            "DIFF-CONTEXT\tafter\t{}\tequal={}\tvendor={}\trust={}",
            item.index,
            item.equal,
            trace_item_text(item.vendor.as_ref()),
            trace_item_text(item.rust.as_ref())
        );
    }
    if let Some(path) = &difference.path {
        let _ = writeln!(
            &mut output,
            "DIFF-PATH\tvendor-branches={}\trust-branches={}\tvendor-calls={}\trust-calls={}",
            path.vendor.branches.len(),
            path.rust.branches.len(),
            path.vendor.calls.len(),
            path.rust.calls.len()
        );
    }
}

fn render_coverage(mut output: &mut String, side: &str, coverage: &CoverageReport) {
    for call in &coverage.covered_calls {
        let _ = writeln!(&mut output, "COVERED-CALL\t{side}\t{call}");
    }
    for outcome in &coverage.branch_outcomes {
        let _ = writeln!(
            &mut output,
            "{}\t{side}\t{}\ttaken={}",
            if outcome.covered {
                "COVERED-BRANCH"
            } else {
                "UNCOVERED-BRANCH"
            },
            outcome.location,
            outcome.taken
        );
    }
    let sites = coverage
        .branch_outcomes
        .iter()
        .map(|outcome| outcome.site)
        .collect::<std::collections::BTreeSet<_>>()
        .len();
    let uncovered = coverage.uncovered_branch_outcomes();
    let _ = writeln!(
        &mut output,
        "SUMMARY-BRANCHES\t{side}\tsites={sites}\toutcomes={}\tcovered={}\tuncovered={uncovered}",
        coverage.branch_outcomes.len(),
        coverage.branch_outcomes.len() - uncovered
    );
    for edge in &coverage.unresolved_control_flow {
        if edge.covered {
            let _ = writeln!(
                &mut output,
                "COVERED-CONTROL-FLOW\t{side}\t{}\ttargets={}",
                edge.location,
                edge.targets.join(",")
            );
        } else {
            let _ = writeln!(
                &mut output,
                "UNCOVERED-CONTROL-FLOW\t{side}\t{}\t{}",
                edge.location, edge.edge
            );
        }
    }
    for address in &coverage.unnamed_mmio {
        let _ = writeln!(&mut output, "UNNAMED-MMIO\t{side}\t{address:#010x}");
    }
}

pub(crate) fn print_execution_comparison(report: &ExecutionComparisonReport) {
    let mut output = String::new();
    let _ = writeln!(
        &mut output,
        "ORACLE\t{}\tsha256={}",
        report.vendor.path, report.vendor.sha256
    );
    if let Some(companion) = &report.vendor.companion {
        let _ = writeln!(
            &mut output,
            "ORACLE\t{}\tsha256={}",
            companion.path, companion.sha256
        );
    }
    for case in &report.cases {
        match case {
            CaseReport::Match {
                name,
                environment,
                events,
                memory_changes,
                return_compared,
            } => {
                render_table_environment(&mut output, name, environment);
                let _ = writeln!(
                    &mut output,
                    "CASE\t{name}\tMATCH\tevents={events}\tmemory-changes={memory_changes}\treturn={}",
                    if *return_compared {
                        "checked"
                    } else {
                        "ignored"
                    }
                );
            }
            CaseReport::Incomplete {
                name,
                environment,
                vendor_error,
                rust_error,
            } => {
                render_table_environment(&mut output, name, environment);
                let _ = writeln!(
                    &mut output,
                    "CASE\t{name}\tINCOMPLETE\tvendor={}\trust={}",
                    vendor_error.as_deref().unwrap_or("complete"),
                    rust_error.as_deref().unwrap_or("complete")
                );
            }
            CaseReport::Diff {
                name,
                environment,
                difference,
            } => {
                render_table_environment(&mut output, name, environment);
                let _ = writeln!(
                    &mut output,
                    "CASE\t{name}\tDIFF\tkind={}\tfirst-difference={}",
                    difference.kind.label(),
                    difference.first_difference,
                );
                render_difference(&mut output, name, difference);
            }
        }
    }
    if let Some(gap) = &report.coverage_gap {
        render_difference(&mut output, "coverage", gap);
    }
    render_coverage(&mut output, "vendor", &report.vendor_coverage);
    render_coverage(&mut output, "rust", &report.rust_coverage);
    let summary = &report.summary;
    let _ = writeln!(
        &mut output,
        "SUMMARY\tcases={}\tmatched={}\tdifferent={}\tincomplete={}\tvendor-uncovered-branch-outcomes={}\trust-uncovered-branch-outcomes={}\tvendor-unresolved-control-flow={}\trust-unresolved-control-flow={}\tvendor-unnamed-mmio={}\trust-unnamed-mmio={}",
        summary.cases,
        summary.matched,
        summary.different,
        summary.incomplete,
        summary.vendor_uncovered_branch_outcomes,
        summary.rust_uncovered_branch_outcomes,
        summary.vendor_unresolved_control_flow,
        summary.rust_unresolved_control_flow,
        summary.vendor_unnamed_mmio,
        summary.rust_unnamed_mmio
    );
    let _ = writeln!(
        &mut output,
        "VERDICT\tmode={}\t{}",
        report.mode.label(),
        report.verdict.label()
    );
    crate::cli::output::text(output);
}

fn render_table_environment(
    mut output: &mut String,
    case: &str,
    environment: &ScenarioEnvironmentReport,
) {
    for (side, instances) in [
        ("vendor", &environment.vendor_tables),
        ("rust", &environment.rust_tables),
    ] {
        for instance in instances {
            let _ = writeln!(
                &mut output,
                "TABLE-INSTANCE\tcase={case}\tside={side}\tlayout={}\tbase={:#010x}\tsize={:#x}\tpointer-cells={}\tslots={}",
                instance.layout_id,
                instance.base_address,
                instance.layout_size,
                instance.pointer_cells.len(),
                instance.slots.len(),
            );
        }
    }
    for device in &environment.device_models {
        let _ = writeln!(
            &mut output,
            "DEVICE-MODEL\tcase={case}\tid={}\tkind={}\tstart={:#010x}\tlength={:#x}",
            device.id, device.kind, device.start, device.length,
        );
    }
    for (side, coverage) in [
        ("vendor", &environment.vendor_device_coverage),
        ("rust", &environment.rust_device_coverage),
    ] {
        for model in coverage {
            let _ = writeln!(
                &mut output,
                "DEVICE-COVERAGE\tcase={case}\tside={side}\tid={}\tkind={}\tcomplete={}\treason={}",
                model.id,
                model.kind,
                model.complete,
                model.reason.as_deref().unwrap_or("-"),
            );
        }
    }
    for (side, instances) in [
        ("vendor", &environment.vendor_memory_instances),
        ("rust", &environment.rust_memory_instances),
    ] {
        for instance in instances {
            let _ = writeln!(
                &mut output,
                "MEMORY-INSTANCE\tcase={case}\tside={side}\tid={}\tbase={:#010x}\tlength={:#x}\tbindings={}",
                instance.id,
                instance.base_address,
                instance.length,
                instance.bindings.len(),
            );
        }
    }
    for (side, events, complete) in [
        (
            "vendor",
            &environment.vendor_table_lifecycle,
            environment.vendor_table_lifecycle_complete,
        ),
        (
            "rust",
            &environment.rust_table_lifecycle,
            environment.rust_table_lifecycle_complete,
        ),
    ] {
        if let Some(complete) = complete {
            let _ = writeln!(
                &mut output,
                "TABLE-LIFECYCLE\tcase={case}\tside={side}\tcomplete={complete}\tevents={}",
                events.len()
            );
        }
    }
}
