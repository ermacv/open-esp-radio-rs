//! Stateless rendering of a browser snapshot.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table, Tabs, Wrap},
};

use super::state::{BrowserState, Section};
use crate::{
    CaseReport, DiagnosticSeverity, EquivalenceVerdict, ExecutionEventReport, TraceItemReport,
    WorkspaceReadiness,
};

const SELECTED: Style = Style::new()
    .fg(Color::Black)
    .bg(Color::Cyan)
    .add_modifier(Modifier::BOLD);

pub(super) fn render(frame: &mut Frame<'_>, state: &BrowserState) {
    let [header, tabs, content, footer] = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3),
            Constraint::Length(3),
            Constraint::Min(8),
            Constraint::Length(2),
        ])
        .areas(frame.area());
    render_header(frame, state, header);
    render_tabs(frame, state, tabs);
    match state.section {
        Section::Overview => render_overview(frame, state, content),
        Section::Functions => render_functions(frame, state, content),
        Section::Registers => render_registers(frame, state, content),
        Section::Interfaces => render_interfaces(frame, state, content),
        Section::Comparisons => render_comparisons(frame, state, content),
        Section::Diagnostics => render_diagnostics(frame, state, content),
        Section::Types => render_types(frame, state, content),
    }
    render_footer(frame, state, footer);
}

fn render_comparisons(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let rows = state
        .snapshot
        .comparisons
        .iter()
        .enumerate()
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
                    " Concrete comparisons ({}) ",
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
                            "evidence: table lifecycle vendor={} rust={}; device coverage vendor={} rust={}",
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
        detail_paragraph(" Trace, evidence and blockers ", lines),
        detail,
    );
}

fn trace_item(item: Option<&TraceItemReport>) -> String {
    match item {
        None => "<missing>".to_owned(),
        Some(TraceItemReport::Event { event }) => match event {
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
        },
        Some(TraceItemReport::Memory { change }) => format!(
            "RAM {:#010x} {:02x}->{:02x}",
            change.address, change.before, change.after
        ),
        Some(TraceItemReport::ReturnValue { value }) => format!("RETURN {value:#010x}"),
        Some(TraceItemReport::Coverage { issue }) => format!("COVERAGE {issue}"),
    }
}

fn render_header(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let project = &state.snapshot.project_status;
    let readiness = readiness(project.overall);
    let line = Line::from(vec![
        Span::styled(
            "Vendor Binary Workbench",
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::raw(&project.project_id),
        Span::raw("  target="),
        Span::raw(&project.target_id),
        Span::raw("  "),
        Span::styled(readiness.0, Style::new().fg(readiness.1)),
        Span::raw(format!("  generation={}", state.snapshot.generation)),
    ]);
    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn render_tabs(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let titles = Section::ALL
        .iter()
        .map(|section| Line::from(section.title()))
        .collect::<Vec<_>>();
    let selected = Section::ALL
        .iter()
        .position(|section| *section == state.section)
        .unwrap_or_default();
    frame.render_widget(
        Tabs::new(titles)
            .select(selected)
            .block(Block::default().borders(Borders::ALL))
            .highlight_style(Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD))
            .divider(" │ "),
        area,
    );
}

fn render_overview(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let rows = state
        .snapshot
        .project_status
        .phases
        .iter()
        .enumerate()
        .map(|(index, phase)| {
            let readiness = readiness(phase.status);
            Row::new([
                Cell::from(phase.name.clone()),
                Cell::from(readiness.0).style(Style::new().fg(readiness.1)),
                Cell::from(phase.components.len().to_string()),
            ])
            .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(52),
                Constraint::Percentage(30),
                Constraint::Percentage(18),
            ],
        )
        .header(Row::new(["Phase", "Status", "Parts"]).style(heading()))
        .block(
            Block::default()
                .title(" Project lifecycle ")
                .borders(Borders::ALL),
        ),
        list,
    );

    let lines = state
        .snapshot
        .project_status
        .phases
        .get(state.selected())
        .map(|phase| {
            phase
                .components
                .iter()
                .flat_map(|component| {
                    let readiness = readiness(component.status);
                    let mut lines = vec![Line::from(vec![
                        Span::styled(&component.name, Style::new().add_modifier(Modifier::BOLD)),
                        Span::raw("  "),
                        Span::styled(readiness.0, Style::new().fg(readiness.1)),
                    ])];
                    if let Some(diagnostic) = &component.diagnostic {
                        lines.push(Line::from(Span::styled(
                            diagnostic,
                            Style::new().fg(Color::Yellow),
                        )));
                    }
                    lines
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![Line::from("No lifecycle phases")]);
    frame.render_widget(detail_paragraph(" Components ", lines), detail);
}

fn render_functions(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let items = state
        .snapshot
        .functions
        .iter()
        .enumerate()
        .map(|(index, function)| {
            ListItem::new(Line::from(vec![
                Span::raw(&function.symbol),
                Span::raw("  "),
                Span::styled(
                    &function.review_status,
                    Style::new().fg(if function.complete {
                        Color::Green
                    } else {
                        Color::Yellow
                    }),
                ),
            ]))
            .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(format!(" Functions ({}) ", state.snapshot.functions.len()))
                .borders(Borders::ALL),
        ),
        list,
    );

    let lines = state
        .snapshot
        .functions
        .get(state.selected())
        .map(|function| {
            let mut lines = vec![
                field("Identity", &function.identity),
                field("Source", &function.source),
                field("Profile", &function.profile),
                field("Selection", &function.selection),
                field("Calls", function.calls.to_string()),
                field("Contexts", function.contexts.len().to_string()),
                field("Memory fields", function.memory_fields.len()),
            ];
            if let Some(role) = &function.role {
                lines.push(field("Role", role));
            }
            if let Some(summary) = &function.summary {
                lines.push(Line::from(""));
                lines.push(Line::from(summary.clone()));
            }
            if !function.blockers.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Blockers",
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )));
                lines.extend(
                    function
                        .blockers
                        .iter()
                        .map(|blocker| Line::from(format!("- {blocker}"))),
                );
            }
            if !function.memory_fields.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Memory objects",
                    Style::new().add_modifier(Modifier::BOLD),
                )));
                lines.extend(function.memory_fields.iter().map(|field| {
                    Line::from(format!(
                        "- {} {:+#x}/{} read={} write={} mask={:#010x}",
                        field.object,
                        field.offset,
                        field.width,
                        field.reads,
                        field.writes,
                        field.write_mask
                    ))
                }));
            }
            if !function.scenario_suggestions.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Scenario candidates (replay required)",
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                )));
                for suggestion in &function.scenario_suggestions {
                    lines.push(Line::from(format!(
                        "- {} @ {}: {}",
                        suggestion.kind,
                        suggestion
                            .site
                            .map_or_else(|| "-".to_owned(), |site| format!("{site:#010x}")),
                        suggestion.evidence
                    )));
                    for variant in &suggestion.variants {
                        let arguments = variant
                            .arguments
                            .iter()
                            .map(|argument| format!("a{}={:#010x}", argument.index, argument.value))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let reads = variant
                            .mmio_reads
                            .iter()
                            .map(|read| {
                                format!(
                                    "read {:#010x}=[{}]",
                                    read.address,
                                    read.values
                                        .iter()
                                        .map(|value| format!("{value:#010x}"))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("; ");
                        lines.push(Line::from(format!(
                            "    {}  {}{}{}",
                            variant.name,
                            arguments,
                            if arguments.is_empty() || reads.is_empty() {
                                ""
                            } else {
                                "; "
                            },
                            reads
                        )));
                    }
                }
            }
            if !function.pseudo_rust.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Pseudo-Rust",
                    Style::new().add_modifier(Modifier::BOLD),
                )));
                lines.extend(
                    function
                        .pseudo_rust
                        .lines()
                        .map(|line| Line::from(line.to_owned())),
                );
            }
            lines
        })
        .unwrap_or_else(|| vec![Line::from("Function IR/review facts are not available")]);
    frame.render_widget(detail_paragraph(" Function detail ", lines), detail);
}

fn render_registers(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let rows = state
        .snapshot
        .registers
        .registers
        .iter()
        .enumerate()
        .map(|(index, register)| {
            Row::new([format!("0x{:08x}", register.address), register.name.clone()])
                .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        Table::new(rows, [Constraint::Length(12), Constraint::Min(12)])
            .header(Row::new(["Address", "Name"]).style(heading()))
            .block(
                Block::default()
                    .title(format!(
                        " Register catalog ({}) ",
                        state.snapshot.registers.registers.len()
                    ))
                    .borders(Borders::ALL),
            ),
        list,
    );
    let report = &state.snapshot.registers;
    let mut lines = vec![
        field("Configured", report.configured),
        field("Ranges", report.ranges),
        field("Observed", report.observed),
        field("Reviewed", report.reviewed),
        field("Manual", report.manual),
        field("Unreviewed", report.unreviewed),
        field("Fields", report.fields),
    ];
    if let Some(model) = &report.model {
        lines.push(field("Model", model.display()));
    }
    if let Some(register) = report.registers.get(state.selected()) {
        lines.push(Line::from(""));
        lines.push(field(
            "Selected address",
            format!("0x{:08x}", register.address),
        ));
        lines.push(field("Selected name", &register.name));
    }
    frame.render_widget(detail_paragraph(" Register workspace ", lines), detail);
}

fn render_interfaces(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let rows = state
        .snapshot
        .interfaces
        .slots
        .iter()
        .enumerate()
        .map(|(index, slot)| {
            Row::new([
                slot.name.clone(),
                format!("{:+#x}", slot.offset),
                slot.semantic.clone().unwrap_or_else(|| "-".to_owned()),
            ])
            .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(40),
                Constraint::Length(10),
                Constraint::Percentage(60),
            ],
        )
        .header(Row::new(["Slot", "Offset", "Semantic"]).style(heading()))
        .block(
            Block::default()
                .title(format!(
                    " Resolved slots ({}) ",
                    state.snapshot.interfaces.slots.len()
                ))
                .borders(Borders::ALL),
        ),
        list,
    );
    let lines = state
        .snapshot
        .interfaces
        .slots
        .get(state.selected())
        .map(|slot| {
            let mut lines = vec![
                field("ID", &slot.id),
                field("Contract", &slot.contract),
                field(
                    "ABI",
                    format!("({}) -> {}", slot.arguments.join(", "), slot.return_type),
                ),
                field("Width", slot.width),
                field("Variadic", slot.variadic),
                field("Call sites", slot.call_sites.len()),
                field("Functions", slot.functions.len()),
            ];
            if let Some(model) = &slot.execution_model {
                lines.push(field("Execution model", model));
            }
            if let Some(replacement) = &slot.replacement {
                lines.push(field("Replacement", replacement));
            }
            if !slot.effects.is_empty() {
                lines.push(field("Effects", slot.effects.join(", ")));
            }
            lines
        })
        .unwrap_or_else(|| vec![Line::from("Interface facts/review pack are not available")]);
    frame.render_widget(detail_paragraph(" Interface detail ", lines), detail);
}

fn render_diagnostics(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let items = state
        .snapshot
        .diagnostics
        .iter()
        .enumerate()
        .map(|(index, diagnostic)| {
            let color = match diagnostic.severity {
                DiagnosticSeverity::Warning => Color::Yellow,
                DiagnosticSeverity::Error => Color::Red,
            };
            ListItem::new(Line::from(vec![
                Span::styled(&diagnostic.component, Style::new().fg(color)),
                Span::raw("  "),
                Span::raw(&diagnostic.message),
            ]))
            .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(format!(
                    " Diagnostics ({}) ",
                    state.snapshot.diagnostics.len()
                ))
                .borders(Borders::ALL),
        ),
        list,
    );
    let lines = state
        .snapshot
        .diagnostics
        .get(state.selected())
        .map(|diagnostic| {
            let mut lines = vec![
                field("Severity", format!("{:?}", diagnostic.severity)),
                field("Component", &diagnostic.component),
                Line::from(""),
                Line::from(diagnostic.message.clone()),
            ];
            if let Some(path) = &diagnostic.path {
                lines.push(Line::from(""));
                lines.push(field("Path", path.display()));
            }
            lines
        })
        .unwrap_or_else(|| vec![Line::from("No diagnostics")]);
    frame.render_widget(detail_paragraph(" Diagnostic detail ", lines), detail);
}

fn render_types(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let rows = state
        .snapshot
        .logical_types
        .iter()
        .enumerate()
        .map(|(index, logical_type)| {
            Row::new([
                logical_type.name.clone(),
                logical_type.bindings.len().to_string(),
                logical_type.fields.len().to_string(),
            ])
            .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(60),
                Constraint::Percentage(20),
                Constraint::Percentage(20),
            ],
        )
        .header(Row::new(["Logical type", "Bindings", "Fields"]).style(heading()))
        .block(
            Block::default()
                .title(format!(
                    " Reviewed types ({}) ",
                    state.snapshot.logical_types.len()
                ))
                .borders(Borders::ALL),
        ),
        list,
    );
    let lines = state
        .snapshot
        .logical_types
        .get(state.selected())
        .map(|logical_type| {
            let mut lines = vec![
                field("ID", &logical_type.id),
                field("Name", &logical_type.name),
            ];
            if let Some(description) = &logical_type.description {
                lines.push(field("Description", description));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Bindings",
                Style::new().add_modifier(Modifier::BOLD),
            )));
            lines.extend(logical_type.bindings.iter().map(|binding| {
                Line::from(format!(
                    "- {} = {} [{} / {}]",
                    binding.name, binding.object, binding.profile, binding.source
                ))
            }));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Fields",
                Style::new().add_modifier(Modifier::BOLD),
            )));
            lines.extend(logical_type.fields.iter().map(|field| {
                Line::from(format!(
                    "- {:+#x}/{} {} {}: {}",
                    field.offset,
                    field.width,
                    field.status,
                    field.name.as_deref().unwrap_or("<unnamed>"),
                    field.display_type.as_deref().unwrap_or("-")
                ))
            }));
            lines
        })
        .unwrap_or_else(|| vec![Line::from("No reviewed logical types")]);
    frame.render_widget(detail_paragraph(" Type unification ", lines), detail);
}

fn render_footer(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let status = state.message.as_deref().unwrap_or(if state.busy {
        "Working..."
    } else {
        "Tab/←/→ section  j/k select  Enter/c compare  r reload  q quit"
    });
    frame.render_widget(
        Paragraph::new(status)
            .alignment(Alignment::Center)
            .style(Style::new().fg(if state.busy {
                Color::Yellow
            } else {
                Color::DarkGray
            })),
        area,
    );
}

fn columns(area: Rect) -> [Rect; 2] {
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .areas(area)
}

fn detail_paragraph<'a>(title: &'a str, lines: Vec<Line<'a>>) -> Paragraph<'a> {
    Paragraph::new(lines)
        .block(Block::default().title(title).borders(Borders::ALL))
        .wrap(Wrap { trim: false })
}

fn heading() -> Style {
    Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan)
}

fn selected_style(index: usize, selected: usize) -> Style {
    if index == selected {
        SELECTED
    } else {
        Style::default()
    }
}

fn readiness(value: WorkspaceReadiness) -> (&'static str, Color) {
    match value {
        WorkspaceReadiness::Ready => ("ready", Color::Green),
        WorkspaceReadiness::Incomplete => ("incomplete", Color::Yellow),
        WorkspaceReadiness::NotConfigured => ("not configured", Color::DarkGray),
        WorkspaceReadiness::Invalid => ("invalid", Color::Red),
    }
}

fn field<'a>(name: &'a str, value: impl ToString) -> Line<'a> {
    Line::from(vec![
        Span::styled(
            format!("{name}: "),
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::raw(value.to_string()),
    ])
}

#[cfg(test)]
mod tests {
    use ratatui::{Terminal, backend::TestBackend};

    use super::*;
    use crate::{
        ComparisonProfileSummary, FunctionSummary, InterfaceWorkspaceReport, ProjectStatusSnapshot,
        RegisterWorkspaceReport, ScenarioArgumentSummary, ScenarioSuggestionSummary,
        ScenarioSuggestionVariantSummary, WorkspacePhaseSnapshot, WorkspaceSnapshot,
    };

    #[test]
    fn overview_renders_from_the_typed_snapshot() {
        let snapshot = WorkspaceSnapshot {
            generation: 7,
            project_status: ProjectStatusSnapshot {
                project_id: "fixture-project".to_owned(),
                manifest: "vendor-project.toml".to_owned(),
                target_id: "fixture-target".to_owned(),
                architecture: "riscv32".to_owned(),
                calling_convention: "riscv-ilp32".to_owned(),
                harness: None,
                overall: WorkspaceReadiness::Incomplete,
                phases: vec![WorkspacePhaseSnapshot {
                    name: "analysis".to_owned(),
                    status: WorkspaceReadiness::Ready,
                    components: Vec::new(),
                }],
            },
            functions: Vec::new(),
            logical_types: Vec::new(),
            registers: RegisterWorkspaceReport {
                configured: false,
                model: None,
                ranges: 0,
                observed: 0,
                reviewed: 0,
                manual: 0,
                unreviewed: 0,
                fields: 0,
                registers: Vec::new(),
            },
            interfaces: InterfaceWorkspaceReport {
                configured: false,
                facts: None,
                pack: None,
                observed_slots: 0,
                reviewed_slots: 0,
                unreviewed_slots: 0,
                contracts: Vec::new(),
                slots: Vec::new(),
            },
            comparisons: vec![ComparisonProfileSummary {
                name: "trace-init".to_owned(),
                path: "profiles/init.profile".into(),
                vendor_source: "rom".to_owned(),
                vendor_symbol: "phy_init".to_owned(),
                rust_symbol: "open_phy_init".to_owned(),
                scenarios: 2,
            }],
            diagnostics: Vec::new(),
        };
        let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
        let mut state = BrowserState::new(snapshot);
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("Vendor Binary Workbench"));
        assert!(rendered.contains("fixture-project"));
        assert!(rendered.contains("analysis"));
        assert!(rendered.contains("generation=7"));

        state.snapshot.functions.push(FunctionSummary {
            profile: "phy".to_owned(),
            source: "rom".to_owned(),
            identity: "rom::phy_init".to_owned(),
            symbol: "phy_init".to_owned(),
            member: None,
            selection: "symbol-prefix-root".to_owned(),
            review_status: "unreviewed".to_owned(),
            reviewed_name: None,
            role: None,
            summary: None,
            complete: false,
            blockers: Vec::new(),
            semantic_operations: Vec::new(),
            calls: 0,
            contexts: Vec::new(),
            memory_fields: Vec::new(),
            scenario_suggestions: vec![ScenarioSuggestionSummary {
                kind: "argument-branch".to_owned(),
                site: Some(0x1010),
                evidence: "arg0 equal 0x1".to_owned(),
                variants: vec![ScenarioSuggestionVariantSummary {
                    name: "branch-taken".to_owned(),
                    arguments: vec![ScenarioArgumentSummary { index: 0, value: 1 }],
                    mmio_reads: Vec::new(),
                }],
            }],
            pseudo_rust: String::new(),
        });
        state.section = Section::Functions;
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("Scenario candidates (replay required)"));
        assert!(rendered.contains("a0=0x00000001"));

        state.section = Section::Comparisons;
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("trace-init"));
        assert!(rendered.contains("Press Enter or c"));
    }
}
