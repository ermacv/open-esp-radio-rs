//! Stateless rendering of a browser snapshot.

use ratatui::{
    Frame,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, List, ListItem, Paragraph, Row, Table, Tabs, Wrap},
};

use super::state::{BrowserState, Section};
use crate::{DiagnosticSeverity, Readiness};

mod blockers;
mod code;
mod comparisons;
mod functions;
mod interfaces;
mod policy;
mod registers;
mod scopes;

const SELECTED: Style = Style::new()
    .fg(Color::Black)
    .bg(Color::Cyan)
    .add_modifier(Modifier::BOLD);

pub(super) fn render(frame: &mut Frame<'_>, state: &BrowserState) {
    if frame.area().width < 64 || frame.area().height < 16 {
        frame.render_widget(
            Paragraph::new("Vendor Binary Workbench needs at least a 64×16 terminal")
                .alignment(Alignment::Center)
                .wrap(Wrap { trim: true }),
            frame.area(),
        );
        return;
    }
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
        Section::Policy => policy::render(frame, state, content),
        Section::Scopes => scopes::render(frame, state, content),
        Section::Code => code::render(frame, state, content),
        Section::Functions => functions::render(frame, state, content),
        Section::Blockers => blockers::render(frame, state, content),
        Section::Registers => registers::render(frame, state, content),
        Section::Interfaces => interfaces::render(frame, state, content),
        Section::Comparisons => comparisons::render(frame, state, content),
        Section::Diagnostics => render_diagnostics(frame, state, content),
        Section::Types => render_types(frame, state, content),
    }
    render_footer(frame, state, footer);
}

fn render_header(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let project = &state.snapshot.project_status;
    let readiness = readiness(project.overall);
    let mut spans = vec![
        Span::styled(
            if area.width < 100 {
                "Workbench"
            } else {
                "Vendor Binary Workbench"
            },
            Style::new().add_modifier(Modifier::BOLD),
        ),
        Span::raw("  "),
        Span::raw(&project.project_id),
        Span::raw("  "),
        Span::styled(readiness.0, Style::new().fg(readiness.1)),
    ];
    if area.width >= 100 {
        spans.extend([
            Span::raw("  target="),
            Span::raw(&project.target.id),
            Span::raw(format!("  generation={}", state.snapshot.generation)),
        ]);
    }
    let line = Line::from(spans);
    frame.render_widget(
        Paragraph::new(line).block(Block::default().borders(Borders::ALL)),
        area,
    );
}

fn render_tabs(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let compact = area.width < 110;
    let titles = Section::ALL
        .iter()
        .map(|section| Line::from(section_title(*section, compact)))
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
            .divider(if compact { " " } else { " │ " }),
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
        .filter(|(index, _)| state.is_visible(*index))
        .skip(state.viewport_start(table_rows(list)))
        .take(table_rows(list))
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
                Constraint::Min(12),
                Constraint::Length(12),
                Constraint::Length(5),
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
                    if let Some(action) = &component.next_action {
                        lines.push(Line::from(vec![
                            Span::styled("Next: ", Style::new().fg(Color::Cyan)),
                            Span::raw(action),
                        ]));
                    }
                    lines
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_else(|| vec![Line::from("No lifecycle phases")]);
    frame.render_widget(
        detail_paragraph(" Components ", lines, state.detail_scroll()),
        detail,
    );
}

fn render_diagnostics(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let items = state
        .snapshot
        .diagnostics
        .iter()
        .enumerate()
        .filter(|(index, _)| state.is_visible(*index))
        .skip(state.viewport_start(list_rows(list)))
        .take(list_rows(list))
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
                    " Diagnostics ({}/{}) ",
                    state.visible_count(),
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
    frame.render_widget(
        detail_paragraph(" Diagnostic detail ", lines, state.detail_scroll()),
        detail,
    );
}

fn render_types(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let rows = state
        .snapshot
        .logical_types
        .iter()
        .enumerate()
        .filter(|(index, _)| state.is_visible(*index))
        .skip(state.viewport_start(table_rows(list)))
        .take(table_rows(list))
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
                    " Reviewed types ({}/{}) ",
                    state.visible_count(),
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
                    field.status.label(),
                    field.name.as_deref().unwrap_or("<unnamed>"),
                    field.display_type.as_deref().unwrap_or("-")
                ))
            }));
            lines
        })
        .unwrap_or_else(|| vec![Line::from("No reviewed logical types")]);
    frame.render_widget(
        detail_paragraph(" Type unification ", lines, state.detail_scroll()),
        detail,
    );
}

fn render_footer(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let status = if state.search_editing {
        format!("Search: {}_", state.search_query)
    } else if !state.search_query.is_empty() {
        format!(
            "Filter: {}  / edit  Esc clear  PgUp/PgDn detail",
            state.search_query
        )
    } else if let Some(message) = &state.message {
        message.clone()
    } else if state.busy {
        "Working...".to_owned()
    } else {
        if area.width < 100 {
            "Tab tabs  j/k move  / find  d/u detail  Enter open  c compare  r reload  q quit"
                .to_owned()
        } else {
            "Tab/←/→ section  j/k select  / search  PgUp/PgDn detail  Enter/c compare  r reload  q quit"
                .to_owned()
        }
    };
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

fn section_title(section: Section, compact: bool) -> &'static str {
    if !compact {
        return section.title();
    }
    match section {
        Section::Overview => "Home",
        Section::Policy => "Policy",
        Section::Scopes => "Scope",
        Section::Code => "Code",
        Section::Functions => "Func",
        Section::Blockers => "Block",
        Section::Registers => "Regs",
        Section::Interfaces => "API",
        Section::Comparisons => "Diff",
        Section::Diagnostics => "Diag",
        Section::Types => "Type",
    }
}

pub(super) fn columns(area: Rect) -> [Rect; 2] {
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .areas(area)
}

pub(super) fn detail_paragraph<'a>(
    title: &'a str,
    lines: Vec<Line<'a>>,
    scroll: u16,
) -> Paragraph<'a> {
    Paragraph::new(lines)
        .block(Block::default().title(title).borders(Borders::ALL))
        .wrap(Wrap { trim: false })
        .scroll((scroll, 0))
}

pub(super) fn list_rows(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(2)).max(1)
}

pub(super) fn table_rows(area: Rect) -> usize {
    usize::from(area.height.saturating_sub(3)).max(1)
}

pub(super) fn heading() -> Style {
    Style::new().add_modifier(Modifier::BOLD).fg(Color::Cyan)
}

pub(super) fn selected_style(index: usize, selected: usize) -> Style {
    if index == selected {
        SELECTED
    } else {
        Style::default()
    }
}

fn readiness(value: Readiness) -> (&'static str, Color) {
    match value {
        Readiness::Ready => ("ready", Color::Green),
        Readiness::Inventory => ("inventory", Color::Cyan),
        Readiness::Incomplete => ("incomplete", Color::Yellow),
        Readiness::NotConfigured => ("not configured", Color::DarkGray),
        Readiness::Invalid => ("invalid", Color::Red),
    }
}

pub(super) fn field<'a>(name: &'a str, value: impl ToString) -> Line<'a> {
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
        CodeWorkspaceReport, ComparisonProfileSummary, FunctionSummary, InterfaceWorkspaceReport,
        ProjectStatusPhase, ProjectStatusReport, ProjectTargetIdentity, RegisterSummary,
        RegisterWorkspaceReport, ScenarioArgumentSummary, ScenarioSuggestionSummary,
        ScenarioSuggestionVariantSummary, WorkspaceSnapshot,
    };

    #[test]
    fn overview_renders_from_the_typed_snapshot() {
        let snapshot = WorkspaceSnapshot {
            generation: 7,
            project_status: ProjectStatusReport {
                project_id: "fixture-project".to_owned(),
                manifest: "vendor-project.toml".to_owned(),
                target: ProjectTargetIdentity {
                    id: "fixture-target".to_owned(),
                    architecture: "riscv32".to_owned(),
                    calling_convention: "riscv-ilp32".to_owned(),
                    knowledge_provider: None,
                },
                overall: Readiness::Incomplete,
                phases: vec![ProjectStatusPhase {
                    name: "analysis".to_owned(),
                    status: Readiness::Ready,
                    components: Vec::new(),
                }],
            },
            code: CodeWorkspaceReport {
                configured: false,
                facts: None,
                pack: None,
                review_output: None,
                observed_candidates: 0,
                accepted: 0,
                rejected: 0,
                unreviewed: 0,
                boundaries: Vec::new(),
            },
            functions: Vec::new(),
            logical_types: Vec::new(),
            registers: RegisterWorkspaceReport {
                configured: false,
                model: None,
                ranges: 0,
                observed: 0,
                reviewed: 0,
                ignored: 0,
                non_operational: 0,
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
            review_scopes: Vec::new(),
            verification_policy: Vec::new(),
            review_queue: Vec::new(),
            comparisons: vec![ComparisonProfileSummary {
                name: "trace-init".to_owned(),
                path: "profiles/init.toml".into(),
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

        let mut compact_terminal = Terminal::new(TestBackend::new(80, 24)).unwrap();
        compact_terminal
            .draw(|frame| render(frame, &state))
            .unwrap();
        let compact = compact_terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(compact.contains("Workbench"));
        assert!(compact.contains("incomplete"));
        assert!(compact.contains("Diff"));
        assert!(compact.contains("Type"));
        assert!(compact.contains("d/u detail"));
        assert!(compact.contains("q quit"));

        let function = FunctionSummary {
            profile: "phy".to_owned(),
            source: "rom".to_owned(),
            identity: "rom::phy_init".to_owned(),
            symbol: "phy_init".to_owned(),
            member: None,
            selection: crate::FunctionSelection::SymbolPrefixRoot,
            review_status: crate::FunctionReviewState::Unreviewed,
            reviewed_name: None,
            role: None,
            summary: None,
            complete: false,
            blockers: Vec::new(),
            decode_blockers: 1,
            decode_blocker_classes: vec!["zero-fill-or-illegal-trap".to_owned()],
            decode_blocker_operations: vec!["illegal-zero".to_owned()],
            semantic_operations: Vec::new(),
            registers: vec![0x2010_4090],
            mmio_sites: vec![crate::FunctionMmioSiteSummary {
                address: 0x2010_4090,
                width: 32,
                access: "read".to_owned(),
                pc: 0x1002_3562,
            }],
            calls: 0,
        };
        state.function_detail_finished(
            function.identity.clone(),
            Some(crate::FunctionDetailSummary {
                identity: function.identity.clone(),
                registers: Vec::new(),
                contexts: Vec::new(),
                memory_fields: Vec::new(),
                decode_blockers: vec![crate::FunctionDecodeBlockerSummary {
                    address: 0x1020,
                    width: 2,
                    raw: 0,
                    class: "zero-fill-or-illegal-trap".to_owned(),
                    operation: "illegal-zero".to_owned(),
                    linear_control_flow: false,
                }],
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
                profile_draft: Some("profile draft-phy-init".to_owned()),
                pseudo_rust: String::new(),
                reviewed_preconditions: Vec::new(),
                reviewed_paths: Vec::new(),
                investigation: None,
            }),
        );
        state.snapshot.functions.push(function);
        state.section = Section::Functions;
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("0x20104090/32"));
        assert!(rendered.contains("0x10023562"));

        state.scroll_detail_down(4);
        terminal.draw(|frame| render(frame, &state)).unwrap();
        let rendered = terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect::<String>();
        assert!(rendered.contains("zero-fill-or-illegal-trap"));

        state.scroll_detail_down(4);
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

        for index in 1..30_u32 {
            let mut item = state.snapshot.functions[0].clone();
            item.identity = format!("rom::function-{index:02}");
            item.symbol = format!("function-{index:02}");
            state.snapshot.functions.push(item);
            state.snapshot.registers.registers.push(RegisterSummary {
                address: 0x2010_0000 + index * 4,
                name: format!("REGISTER_{index:02}"),
            });
            let mut comparison = state.snapshot.comparisons[0].clone();
            comparison.name = format!("case-{index:02}");
            state.snapshot.comparisons.push(comparison);
        }
        state.snapshot.registers.registers.insert(
            0,
            RegisterSummary {
                address: 0x2010_0000,
                name: "REGISTER_00".to_owned(),
            },
        );

        state.section = Section::Functions;
        state.select_last();
        compact_terminal
            .draw(|frame| render(frame, &state))
            .unwrap();
        assert!(buffer_text(&compact_terminal).contains("function-29"));

        state.section = Section::Registers;
        state.select_last();
        compact_terminal
            .draw(|frame| render(frame, &state))
            .unwrap();
        assert!(buffer_text(&compact_terminal).contains("0x20100074"));

        state.section = Section::Comparisons;
        state.select_last();
        compact_terminal
            .draw(|frame| render(frame, &state))
            .unwrap();
        assert!(buffer_text(&compact_terminal).contains("case-29"));
    }

    fn buffer_text(terminal: &Terminal<TestBackend>) -> String {
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }
}
