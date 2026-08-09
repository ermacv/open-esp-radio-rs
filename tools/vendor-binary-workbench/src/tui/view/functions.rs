//! Function index, reviewed metadata, scenario candidates and pseudo-code view.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
};

use super::super::state::BrowserState;
use super::{columns, detail_paragraph, field, list_rows, selected_style};

pub(super) fn render(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let items = state
        .snapshot
        .functions
        .iter()
        .enumerate()
        .filter(|(index, _)| state.is_visible(*index))
        .skip(state.viewport_start(list_rows(list)))
        .take(list_rows(list))
        .map(|(index, function)| {
            ListItem::new(Line::from(vec![
                Span::raw(&function.symbol),
                Span::raw("  "),
                Span::styled(
                    function.review_status.label(),
                    Style::new().fg(if function.complete {
                        Color::Green
                    } else {
                        Color::Yellow
                    }),
                ),
                Span::raw(if function.decode_blockers == 0 {
                    String::new()
                } else {
                    format!("  decode:{}", function.decode_blockers)
                }),
            ]))
            .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(format!(
                    " Functions ({}/{}) ",
                    state.visible_count(),
                    state.snapshot.functions.len()
                ))
                .borders(Borders::ALL),
        ),
        list,
    );

    let lines = state
        .snapshot
        .functions
        .get(state.selected())
        .map(|function| {
            let function_detail = state.function_detail(&function.identity);
            let mut lines = vec![
                field("Identity", &function.identity),
                field("Source", &function.source),
                field("Profile", &function.profile),
                field("Selection", function.selection.label()),
                field("Calls", function.calls.to_string()),
                field("Decode blockers", function.decode_blockers),
                field("Registers", function.registers.len()),
                field(
                    "Contexts",
                    function_detail.map_or(0, |detail| detail.contexts.len()),
                ),
                field(
                    "Memory fields",
                    function_detail.map_or(0, |detail| detail.memory_fields.len()),
                ),
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
            if let Some(detail) =
                function_detail.filter(|detail| !detail.decode_blockers.is_empty())
            {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Decode blockers",
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )));
                lines.extend(detail.decode_blockers.iter().map(|blocker| {
                    Line::from(format!(
                        "- {:#010x}: {} width={} raw={:#010x} flow={}",
                        blocker.address,
                        blocker.class,
                        blocker.width,
                        blocker.raw,
                        if blocker.linear_control_flow {
                            "linear"
                        } else {
                            "blocked"
                        }
                    ))
                }));
            }
            if let Some(detail) = function_detail.filter(|detail| !detail.memory_fields.is_empty())
            {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Memory objects",
                    Style::new().add_modifier(Modifier::BOLD),
                )));
                lines.extend(detail.memory_fields.iter().map(|field| {
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
            if let Some(detail) =
                function_detail.filter(|detail| !detail.scenario_suggestions.is_empty())
            {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Scenario candidates (replay required)",
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                )));
                for suggestion in &detail.scenario_suggestions {
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
            if let Some(draft) = function_detail.and_then(|detail| detail.profile_draft.as_ref()) {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Editable profile draft (replay required)",
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                )));
                lines.extend(draft.lines().map(|line| Line::from(line.to_owned())));
            }
            if let Some(detail) = function_detail.filter(|detail| !detail.pseudo_rust.is_empty()) {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Pseudo-Rust",
                    Style::new().add_modifier(Modifier::BOLD),
                )));
                lines.extend(
                    detail
                        .pseudo_rust
                        .lines()
                        .map(|line| Line::from(line.to_owned())),
                );
            }
            lines
        })
        .unwrap_or_else(|| vec![Line::from("Function IR/review facts are not available")]);
    frame.render_widget(
        detail_paragraph(" Function detail ", lines, state.detail_scroll()),
        detail,
    );
}
