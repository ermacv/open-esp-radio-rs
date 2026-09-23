//! Shared interface observations and reviewed slot rendering.

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    text::Line,
    widgets::{Block, Borders, Row, Table},
};

use super::{columns, detail_paragraph, field, heading, selected_style, table_rows};
use crate::tui::state::BrowserState;

pub(super) fn render(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let total = crate::tui::interface_rows::count(&state.snapshot.interfaces);
    let selected = state.selected();
    let rows = (0..total)
        .filter(|index| state.is_visible(*index))
        .skip(state.viewport_start(table_rows(list)))
        .take(table_rows(list))
        .filter_map(|index| {
            crate::tui::interface_rows::at(&state.snapshot.interfaces, index)
                .map(|row| Row::new(row.columns()).style(selected_style(index, selected)))
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Min(8),
                Constraint::Length(9),
                Constraint::Length(10),
            ],
        )
        .header(Row::new(["Observation / slot", "Site / offset", "Status"]).style(heading()))
        .block(
            Block::default()
                .title(format!(
                    " Interface evidence ({}/{}) ",
                    state.visible_count(),
                    total
                ))
                .borders(Borders::ALL),
        ),
        list,
    );
    let selected_row = state
        .is_visible(selected)
        .then(|| crate::tui::interface_rows::at(&state.snapshot.interfaces, selected))
        .flatten();
    let lines = match selected_row {
        Some(crate::tui::interface_rows::InterfaceRow::Slot(slot)) => {
            let mut lines = vec![
                field("ID", &slot.id),
                field("Contract", &slot.contract),
                field("Review", slot.review_state.label()),
                field(
                    "ABI",
                    if slot.review_state == crate::InterfaceReviewState::Reviewed {
                        format!("({}) -> {}", slot.arguments.join(", "), slot.return_type)
                    } else {
                        "unknown until reviewed".to_owned()
                    },
                ),
                field("Width", slot.width),
                field("Variadic", slot.variadic),
                field("Call sites", slot.call_sites.len()),
                field("Functions", slot.functions.len()),
            ];
            if let Some(selector) = &slot.selector {
                lines.push(field("Selector", selector));
            }
            if let Some(semantic) = &slot.semantic {
                lines.push(field("Semantic", semantic));
            }
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
        }
        Some(row) => {
            let mut lines = vec![
                field("Evidence", row.columns()[0].clone()),
                field("Behavior", "unknown from this observation alone"),
            ];
            lines.extend(
                row.evidence()
                    .lines()
                    .map(|line| Line::from(line.to_owned())),
            );
            lines
        }
        None => vec![Line::from(
            match &state.snapshot.interfaces.observation_state {
                crate::InterfaceObservationState::NotConfigured => {
                    "Interface observations are not configured".to_owned()
                }
                crate::InterfaceObservationState::Missing => {
                    "Interface observations have not been generated".to_owned()
                }
                crate::InterfaceObservationState::Failed { reason } => {
                    format!("Interface observations unavailable: {reason}")
                }
                crate::InterfaceObservationState::Available => {
                    "No observations match the current view".to_owned()
                }
            },
        )],
    };
    frame.render_widget(
        detail_paragraph(" Interface detail ", lines, state.detail_scroll()),
        detail,
    );
}
