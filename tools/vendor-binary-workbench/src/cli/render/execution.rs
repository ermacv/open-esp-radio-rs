//! Terminal renderer for concrete execution comparisons.

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

fn print_difference(case: &str, difference: &TraceDiffReport) {
    outputln!(
        "FIRST-DIFFERENCE\tcase={case}\tkind={}\tindex={}",
        difference.kind.label(),
        difference.first_difference
    );
    for item in &difference.context_before {
        outputln!(
            "DIFF-CONTEXT\tbefore\t{}\tequal={}\tvendor={}\trust={}",
            item.index,
            item.equal,
            trace_item_text(item.vendor.as_ref()),
            trace_item_text(item.rust.as_ref())
        );
    }
    outputln!(
        "DIFF-ITEM\t{}\tvendor={}\trust={}",
        difference.first_difference,
        trace_item_text(difference.vendor.as_ref()),
        trace_item_text(difference.rust.as_ref())
    );
    for item in &difference.context_after {
        outputln!(
            "DIFF-CONTEXT\tafter\t{}\tequal={}\tvendor={}\trust={}",
            item.index,
            item.equal,
            trace_item_text(item.vendor.as_ref()),
            trace_item_text(item.rust.as_ref())
        );
    }
    if let Some(path) = &difference.path {
        outputln!(
            "DIFF-PATH\tvendor-branches={}\trust-branches={}\tvendor-calls={}\trust-calls={}",
            path.vendor.branches.len(),
            path.rust.branches.len(),
            path.vendor.calls.len(),
            path.rust.calls.len()
        );
    }
}

fn print_coverage(side: &str, coverage: &CoverageReport) {
    for call in &coverage.covered_calls {
        outputln!("COVERED-CALL\t{side}\t{call}");
    }
    for outcome in &coverage.branch_outcomes {
        outputln!(
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
    outputln!(
        "SUMMARY-BRANCHES\t{side}\tsites={sites}\toutcomes={}\tcovered={}\tuncovered={uncovered}",
        coverage.branch_outcomes.len(),
        coverage.branch_outcomes.len() - uncovered
    );
    for edge in &coverage.unresolved_control_flow {
        if edge.covered {
            outputln!(
                "COVERED-CONTROL-FLOW\t{side}\t{}\ttargets={}",
                edge.location,
                edge.targets.join(",")
            );
        } else {
            outputln!(
                "UNCOVERED-CONTROL-FLOW\t{side}\t{}\t{}",
                edge.location,
                edge.edge
            );
        }
    }
    for address in &coverage.unnamed_mmio {
        outputln!("UNNAMED-MMIO\t{side}\t{address:#010x}");
    }
}

pub(crate) fn print_execution_comparison(report: &ExecutionComparisonReport) {
    outputln!(
        "ORACLE\t{}\tsha256={}",
        report.vendor.path,
        report.vendor.sha256
    );
    if let Some(companion) = &report.vendor.companion {
        outputln!("ORACLE\t{}\tsha256={}", companion.path, companion.sha256);
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
                print_table_environment(name, environment);
                outputln!(
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
                print_table_environment(name, environment);
                outputln!(
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
                print_table_environment(name, environment);
                outputln!(
                    "CASE\t{name}\tDIFF\tkind={}\tfirst-difference={}",
                    difference.kind.label(),
                    difference.first_difference,
                );
                print_difference(name, difference);
            }
        }
    }
    if let Some(gap) = &report.coverage_gap {
        print_difference("coverage", gap);
    }
    print_coverage("vendor", &report.vendor_coverage);
    print_coverage("rust", &report.rust_coverage);
    let summary = &report.summary;
    outputln!(
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
    outputln!(
        "VERDICT\tmode={}\t{}",
        report.mode.label(),
        report.verdict.label()
    );
}

fn print_table_environment(case: &str, environment: &ScenarioEnvironmentReport) {
    for (side, instances) in [
        ("vendor", &environment.vendor_tables),
        ("rust", &environment.rust_tables),
    ] {
        for instance in instances {
            outputln!(
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
        outputln!(
            "DEVICE-MODEL\tcase={case}\tid={}\tkind={}\tstart={:#010x}\tlength={:#x}",
            device.id,
            device.kind,
            device.start,
            device.length,
        );
    }
    for (side, coverage) in [
        ("vendor", &environment.vendor_device_coverage),
        ("rust", &environment.rust_device_coverage),
    ] {
        for model in coverage {
            outputln!(
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
            outputln!(
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
            outputln!(
                "TABLE-LIFECYCLE\tcase={case}\tside={side}\tcomplete={complete}\tevents={}",
                events.len()
            );
        }
    }
}
