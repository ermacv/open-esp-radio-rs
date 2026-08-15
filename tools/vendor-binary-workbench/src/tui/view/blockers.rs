//! Scope-prioritized root-cause queue.

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    text::Line,
    widgets::{Block, Borders, Cell, Row, Table},
};

use super::{columns, detail_paragraph, field, selected_style, table_rows};
use crate::tui::state::BrowserState;

pub(super) fn render(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let selected = state.selected();
    let rows = (0..state.snapshot.review_queue.len())
        .filter(|index| state.is_visible(*index))
        .skip(state.viewport_start(table_rows(list)))
        .take(table_rows(list))
        .map(|index| {
            let item = &state.snapshot.review_queue[index];
            Row::new([
                Cell::from(item.priority.to_string()),
                Cell::from(item.scope.clone()),
                Cell::from(item.kind.clone()),
                Cell::from(item.functions.len().to_string()),
            ])
            .style(selected_style(index, selected))
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Length(4),
                Constraint::Percentage(24),
                Constraint::Percentage(54),
                Constraint::Length(5),
            ],
        )
        .header(Row::new(["Pri", "Scope", "Kind", "Fns"]))
        .block(
            Block::default()
                .title(format!(
                    " Review queue ({}/{}) ",
                    state.visible_count(),
                    state.snapshot.review_queue.len()
                ))
                .borders(Borders::ALL),
        ),
        list,
    );

    let lines = state
        .snapshot
        .review_queue
        .get(selected)
        .map(|item| {
            vec![
                field("ID", &item.id),
                field("Scope", &item.scope),
                field("Priority", item.priority),
                field("Severity", &item.severity),
                field("Kind", &item.kind),
                field("Occurrences", item.occurrences),
                field(
                    "Potentially unblocked functions",
                    item.potentially_unblocked_functions,
                ),
                field("Affected roots", item.affected_scope_roots.join(", ")),
                field("Channels", item.channels.join(", ")),
                field(
                    "Sites",
                    item.sites
                        .iter()
                        .map(|site| format!("{site:#010x}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
                field("Functions", item.functions.join(", ")),
                Line::from(""),
                Line::from(item.message.clone()),
            ]
        })
        .unwrap_or_else(|| vec![Line::from("No release-scope blockers")]);
    frame.render_widget(
        detail_paragraph(" Root cause ", lines, state.detail_scroll()),
        detail,
    );
}
