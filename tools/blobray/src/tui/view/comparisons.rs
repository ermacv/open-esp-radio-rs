//! Comparison profile and trace-difference view.

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Row, Table},
};

use super::super::state::BrowserState;
use super::{columns, detail_paragraph, field, heading, selected_style, table_rows};
use crate::{CaseReport, EquivalenceVerdict, ExecutionEventReport, TraceItemReport};

pub(super) fn render(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let rows = state
        .snapshot
        .comparisons
        .iter()
        .enumerate()
        .filter(|(index, _)| state.is_visible(*index))
        .skip(state.viewport_start(table_rows(list)))
        .take(table_rows(list))
        .map(|(index, profile)| {
            let verdict = state
                .comparisons
                .get(&profile.name)
                .map_or("not run", |report| report.verdict.label());
            Row::new([
                profile.name.clone(),
                profile.vendor_source.clone(),
                profile.scenarios.to_string(),
                verdict.to_owned(),
            ])
            .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(50),
                Constraint::Length(10),
                Constraint::Length(7),
                Constraint::Length(12),
            ],
        )
        .header(Row::new(["Profile", "Source", "Cases", "Verdict"]).style(heading()))
        .block(
            Block::default()
                .title(format!(
                    " Concrete comparisons ({}/{}) ",
                    state.visible_count(),
                    state.snapshot.comparisons.len()
                ))
                .borders(Borders::ALL),
        ),
        list,
    );

    let lines = state
        .snapshot
        .comparisons
        .get(state.selected())
        .map(|profile| {
            let mut lines = vec![
                field("Profile", &profile.name),
                field("Manifest", profile.path.display()),
                field("Vendor", format!("{}::{}", profile.vendor_source, profile.vendor_symbol)),
                field("Rust", &profile.rust_symbol),
                field("Scenarios", profile.scenarios),
            ];
            let Some(report) = state.comparisons.get(&profile.name) else {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Press Enter or c to execute this reviewed profile.",
                    Style::new().fg(Color::Cyan),
                )));
                return lines;
            };
            lines.push(Line::from(""));
            lines.push(field("Verdict", report.verdict.label()));
            lines.push(field("Mode", report.mode.label()));
            lines.push(field(
                "Diagnostics",
                format!(
                    "{} ({} contracts)",
                    report
                        .diagnostic_contracts
                        .knowledge_provider
                        .as_deref()
                        .unwrap_or("neutral"),
                    report.diagnostic_contracts.calls.len()
                ),
            ));
            lines.push(field(
                "Summary",
                format!(
                    "match={} diff={} incomplete={}",
                    report.summary.matched, report.summary.different, report.summary.incomplete
                ),
            ));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Artifact provenance",
                Style::new().add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(format!(
                "vendor {}  sha256={}", report.vendor.path, report.vendor.sha256
            )));
            lines.push(Line::from(format!(
                "rust   {}  sha256={}", report.rust.path, report.rust.sha256
            )));

            let vendor_branch_gaps = report.vendor_coverage.uncovered_branch_outcomes();
            let rust_branch_gaps = report.rust_coverage.uncovered_branch_outcomes();
            let vendor_flow_gaps = report.vendor_coverage.uncovered_control_flow();
            let rust_flow_gaps = report.rust_coverage.uncovered_control_flow();
            if vendor_branch_gaps + rust_branch_gaps + vendor_flow_gaps + rust_flow_gaps != 0 {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Coverage blockers",
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )));
                lines.push(Line::from(format!(
                    "branches vendor={vendor_branch_gaps} rust={rust_branch_gaps}; control-flow vendor={vendor_flow_gaps} rust={rust_flow_gaps}"
                )));
            }
            for case in &report.cases {
                lines.push(Line::from(""));
                match case {
                    CaseReport::Match {
                        name,
                        environment,
                        events,
                        memory_changes,
                        ..
                    } => {
                        lines.push(Line::from(Span::styled(
                            format!("{name}: MATCH"),
                            Style::new().fg(Color::Green).add_modifier(Modifier::BOLD),
                        )));
                        lines.push(Line::from(format!(
                            "events={events} memory-changes={memory_changes} tables(v/r)={}/{} devices={}",
                            environment.vendor_tables.len(),
                            environment.rust_tables.len(),
                            environment.device_models.len()
                        )));
                    }
                    CaseReport::Diff {
                        name,
                        environment,
                        difference,
                    } => {
                        lines.push(Line::from(Span::styled(
                            format!("{name}: DIFF"),
                            Style::new().fg(Color::Red).add_modifier(Modifier::BOLD),
                        )));
                        lines.push(Line::from(format!(
                            "first #{} {:?}: vendor={} | rust={}",
                            difference.first_difference,
                            difference.kind,
                            trace_item(difference.vendor.as_ref()),
                            trace_item(difference.rust.as_ref())
                        )));
                        for item in difference
                            .context_before
                            .iter()
                            .chain(&difference.context_after)
                        {
                            lines.push(Line::from(format!(
                                "  #{} {} vendor={} | rust={}",
                                item.index,
                                if item.equal { "=" } else { "!" },
                                trace_item(item.vendor.as_ref()),
                                trace_item(item.rust.as_ref())
                            )));
                        }
                        lines.push(Line::from(format!(
                            "evidence: allocations vendor={} rust={}; table lifecycle vendor={} rust={}; device coverage vendor={} rust={}",
                            environment.vendor_allocations.len(),
                            environment.rust_allocations.len(),
                            environment.vendor_table_lifecycle.len(),
                            environment.rust_table_lifecycle.len(),
                            environment.vendor_device_coverage.len(),
                            environment.rust_device_coverage.len()
                        )));
                    }
                    CaseReport::Incomplete {
                        name,
                        environment,
                        vendor_error,
                        rust_error,
                    } => {
                        lines.push(Line::from(Span::styled(
                            format!("{name}: INCOMPLETE"),
                            Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                        )));
                        lines.push(Line::from(format!(
                            "vendor blocker: {}",
                            vendor_error.as_deref().unwrap_or("none")
                        )));
                        lines.push(Line::from(format!(
                            "rust blocker: {}",
                            rust_error.as_deref().unwrap_or("none")
                        )));
                        for (side, coverage) in [
                            ("vendor", &environment.vendor_device_coverage),
                            ("rust", &environment.rust_device_coverage),
                        ] {
                            for model in coverage.iter().filter(|model| !model.complete) {
                                lines.push(Line::from(format!(
                                    "{side} device {}: {}",
                                    model.id,
                                    model.reason.as_deref().unwrap_or("incomplete")
                                )));
                            }
                        }
                    }
                }
            }
            if report.verdict == EquivalenceVerdict::Match {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "All requested observables and completeness gates matched.",
                    Style::new().fg(Color::Green),
                )));
            }
            lines
        })
        .unwrap_or_else(|| vec![Line::from("No verification profiles are configured")]);
    frame.render_widget(
        detail_paragraph(
            " Trace, evidence and blockers ",
            lines,
            state.detail_scroll(),
        ),
        detail,
    );
}

fn trace_item(item: Option<&TraceItemReport>) -> String {
    match item {
        None => "<missing>".to_owned(),
        Some(TraceItemReport::Transaction { transaction }) => format!("{transaction:?}"),
        Some(TraceItemReport::Event { event, producer }) => {
            let producer = producer.as_ref().map_or_else(String::new, |producer| {
                format!(
                    " [{}+{:#x}@{:#010x}]",
                    producer.symbol.as_deref().unwrap_or("<unknown>"),
                    producer.symbol_offset.unwrap_or_default(),
                    producer.pc
                )
            });
            let event = match event {
                ExecutionEventReport::Read {
                    width,
                    address,
                    register,
                    value,
                    ..
                } => format!(
                    "READ/{width} {}({address:#010x}) -> {value:#010x}",
                    register.as_deref().unwrap_or("<unnamed>")
                ),
                ExecutionEventReport::Write {
                    width,
                    address,
                    register,
                    value,
                    ..
                } => format!(
                    "WRITE/{width} {}({address:#010x}) <- {value:#010x}",
                    register.as_deref().unwrap_or("<unnamed>")
                ),
                ExecutionEventReport::DelayMicros { micros } => format!("DELAY {micros} us"),
                ExecutionEventReport::Fence {
                    predecessor,
                    successor,
                    ..
                } => format!("FENCE {predecessor:#x}->{successor:#x}"),
            };
            format!("{event}{producer}")
        }
        Some(TraceItemReport::Memory { change }) => format!(
            "RAM {:#010x} {:02x}->{:02x}",
            change.address, change.before, change.after
        ),
        Some(TraceItemReport::ReturnValue { value }) => format!("RETURN {value:#010x}"),
        Some(TraceItemReport::Coverage { issue }) => format!("COVERAGE {issue}"),
    }
}
