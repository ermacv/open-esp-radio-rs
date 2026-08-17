//! Reviewed interface slot list and detail rendering.

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
    let rows = state
        .snapshot
        .interfaces
        .slots
        .iter()
        .enumerate()
        .filter(|(index, _)| state.is_visible(*index))
        .skip(state.viewport_start(table_rows(list)))
        .take(table_rows(list))
        .map(|(index, slot)| {
            Row::new([
                slot.name.clone(),
                format!("{:+#x}", slot.offset),
                slot.review_state.label().to_owned(),
            ])
            .style(selected_style(index, state.selected()))
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
        .header(Row::new(["Slot", "Offset", "Review"]).style(heading()))
        .block(
            Block::default()
                .title(format!(
                    " Interface slots ({}/{}) ",
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
        })
        .unwrap_or_else(|| vec![Line::from("Interface facts/review pack are not available")]);
    frame.render_widget(
        detail_paragraph(" Interface detail ", lines, state.detail_scroll()),
        detail,
    );
}
