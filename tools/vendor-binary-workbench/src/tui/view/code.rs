//! Reviewed executable-code boundaries and recovery evidence.

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
        .code
        .boundaries
        .iter()
        .enumerate()
        .filter(|(index, _)| state.is_visible(*index))
        .skip(state.viewport_start(list_rows(list)))
        .take(list_rows(list))
        .map(|(index, boundary)| {
            Row::new([
                boundary
                    .name
                    .clone()
                    .or_else(|| boundary.symbol_names.first().cloned())
                    .unwrap_or_else(|| "<candidate>".to_owned()),
                format!("{:#x}", boundary.address),
                boundary.status.label().to_owned(),
            ])
            .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(46),
                Constraint::Percentage(30),
                Constraint::Percentage(24),
            ],
        )
        .header(Row::new(["Boundary", "Address", "Review"]).style(heading()))
        .block(
            Block::default()
                .title(format!(
                    " Code boundaries ({}/{}) ",
                    state.visible_count(),
                    state.snapshot.code.boundaries.len()
                ))
                .borders(Borders::ALL),
        ),
        list,
    );
    let lines = state
        .snapshot
        .code
        .boundaries
        .get(state.selected())
        .map(|boundary| {
            let mut lines = vec![
                field("Status", boundary.status.label()),
                field("Source", &boundary.source),
                field("Artifact SHA-256", &boundary.artifact_sha256),
                field("Object", &boundary.object_kind),
                field("Section", &boundary.section),
                field("Address", format!("{:#x}", boundary.address)),
                field(
                    "Reviewed range",
                    format!(
                        "{:#x}..{:#x}",
                        boundary.entry_offset, boundary.end_exclusive_offset
                    ),
                ),
                field(
                    "Candidate limit",
                    format!("{:#x}", boundary.end_limit_offset),
                ),
            ];
            if let Some(member) = &boundary.member {
                lines.push(field("Archive member", member));
            }
            if let Some(name) = &boundary.name {
                lines.push(field("Reviewed name", name));
            }
            if let Some(reason) = &boundary.reason {
                lines.push(field("Reason", reason));
            }
            if !boundary.symbol_names.is_empty() {
                lines.push(field("Symbol evidence", boundary.symbol_names.join(", ")));
            }
            lines.extend(boundary.direct_control_flow.iter().map(|edge| {
                Line::from(format!(
                    "- {} from {} at section+{:#x}",
                    edge.kind, edge.caller, edge.site_offset
                ))
            }));
            lines
        })
        .unwrap_or_else(|| {
            vec![Line::from(
                "Generate symbol facts and initialize the reviewed code pack",
            )]
        });
    frame.render_widget(
        detail_paragraph(" Boundary evidence ", lines, state.detail_scroll()),
        detail,
    );
}
