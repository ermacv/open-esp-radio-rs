//! Review-scope overview and navigation.

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
    let rows = (0..state.snapshot.review_scopes.len())
        .filter(|index| state.is_visible(*index))
        .skip(state.viewport_start(table_rows(list)))
        .take(table_rows(list))
        .map(|index| {
            let scope = &state.snapshot.review_scopes[index];
            Row::new([
                Cell::from(scope.id.clone()),
                Cell::from(scope.replacement_coverage.clone()),
                Cell::from(if scope.analysis_inventory_complete {
                    "complete"
                } else {
                    "inventory"
                }),
                Cell::from(scope.blockers.to_string()),
            ])
            .style(selected_style(index, selected))
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(46),
                Constraint::Length(11),
                Constraint::Length(10),
                Constraint::Length(8),
            ],
        )
        .header(Row::new(["Scope", "Replacement", "Analysis", "Queue"]))
        .block(
            Block::default()
                .title(format!(
                    " Review scopes ({}/{}) ",
                    state.visible_count(),
                    state.snapshot.review_scopes.len()
                ))
                .borders(Borders::ALL),
        ),
        list,
    );

    let lines = state
        .snapshot
        .review_scopes
        .get(selected)
        .map(|scope| {
            vec![
                field("ID", &scope.id),
                field("Publication", scope.publication),
                field("Replacement coverage", &scope.replacement_coverage),
                field(
                    "Analysis inventory",
                    if scope.analysis_inventory_complete {
                        "complete"
                    } else {
                        "blocked"
                    },
                ),
                field("Profiles", scope.profiles.join(", ")),
                field("Roots", scope.roots),
                field("Replacement roots", scope.replacement_functions),
                field(
                    "Functions",
                    format!(
                        "{} complete / {}",
                        scope.complete_functions, scope.functions
                    ),
                ),
                field("MMIO registers", scope.mmio_registers),
                field("Table calls", scope.table_calls),
                field("Context fields", scope.context_fields),
                field("Memory fields", scope.memory_fields),
                field("Review queue", scope.blockers),
                field("Decode blockers", scope.decode_blockers),
                field("Unresolved calls", scope.unresolved_calls),
                field("Replacement gaps", scope.replacement_gaps),
                Line::from(""),
                field("Function identities", scope.function_identities.join(", ")),
                field(
                    "MMIO",
                    scope
                        .mmio_addresses
                        .iter()
                        .map(|address| format!("{address:#010x}"))
                        .collect::<Vec<_>>()
                        .join(", "),
                ),
            ]
        })
        .unwrap_or_else(|| vec![Line::from("No generated review scopes")]);
    frame.render_widget(
        detail_paragraph(" Scope evidence ", lines, state.detail_scroll()),
        detail,
    );
}
