//! Reviewed interface slot list and detail rendering.

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    text::Line,
    widgets::{Block, Borders, Row, Table},
};

use super::{columns, detail_paragraph, field, heading, list_rows, selected_style};
use crate::tui::state::BrowserState;

pub(super) fn render(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let rows = state
        .snapshot
        .interfaces
        .slots
        .iter()
        .enumerate()
        .filter(|(index, _)| state.is_visible(*index))
        .skip(state.viewport_start(list_rows(list)))
        .take(list_rows(list))
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
                    " Resolved slots ({}/{}) ",
                    state.visible_count(),
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
    frame.render_widget(
        detail_paragraph(" Interface detail ", lines, state.detail_scroll()),
        detail,
    );
}
