//! Flat verification-policy view.

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
    let rows = (0..state.snapshot.verification_policy.len())
        .filter(|index| state.is_visible(*index))
        .skip(state.viewport_start(table_rows(list)))
        .take(table_rows(list))
        .map(|index| {
            let surface = &state.snapshot.verification_policy[index];
            Row::new([
                Cell::from(surface.id.clone()),
                Cell::from(surface.kind.clone()),
                Cell::from(if surface.closed { "closed" } else { "blocked" }),
                Cell::from(surface.blockers.len().to_string()),
            ])
            .style(selected_style(index, selected))
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(48),
                Constraint::Length(18),
                Constraint::Length(9),
                Constraint::Length(8),
            ],
        )
        .header(Row::new(["Surface", "Kind", "Status", "Blockers"]))
        .block(
            Block::default()
                .title(format!(
                    " Verification policy ({}/{}) ",
                    state.visible_count(),
                    state.snapshot.verification_policy.len()
                ))
                .borders(Borders::ALL),
        ),
        list,
    );

    let lines =
        state
            .snapshot
            .verification_policy
            .get(selected)
            .map(|surface| {
                let mut lines = vec![
                    field("ID", &surface.id),
                    field("Description", &surface.description),
                    field("Kind", &surface.kind),
                    field("Status", if surface.closed { "closed" } else { "blocked" }),
                    field("Review scopes", surface.scopes.join(", ")),
                    field("Requirements", surface.requirements),
                    field("Effects", surface.effects),
                    Line::from(""),
                ];
                if surface.blockers.is_empty() {
                    lines.push(Line::from("No policy blockers"));
                } else {
                    lines.extend(
                        surface.blockers.iter().enumerate().map(|(index, blocker)| {
                            Line::from(format!("{}. {blocker}", index + 1))
                        }),
                    );
                }
                lines
            })
            .unwrap_or_else(|| vec![Line::from("No verification policy surface selected")]);
    frame.render_widget(
        detail_paragraph("Surface", lines, state.detail_scroll()),
        detail,
    );
}
