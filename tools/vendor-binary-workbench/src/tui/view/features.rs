//! Fail-closed feature qualification view.

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
    let rows = (0..state.snapshot.features.len())
        .filter(|index| state.is_visible(*index))
        .skip(state.viewport_start(table_rows(list)))
        .take(table_rows(list))
        .map(|index| {
            let feature = &state.snapshot.features[index];
            Row::new([
                Cell::from(feature.id.clone()),
                Cell::from(if feature.required {
                    "required"
                } else {
                    "review"
                }),
                Cell::from(feature.status.clone()),
                Cell::from(feature.blockers.len().to_string()),
            ])
            .style(selected_style(index, selected))
        });
    frame.render_widget(
        Table::new(
            rows,
            [
                Constraint::Percentage(48),
                Constraint::Length(9),
                Constraint::Length(11),
                Constraint::Length(8),
            ],
        )
        .header(Row::new(["Feature", "Gate", "Status", "Blockers"]))
        .block(
            Block::default()
                .title(format!(
                    " Feature qualification ({}/{}) ",
                    state.visible_count(),
                    state.snapshot.features.len()
                ))
                .borders(Borders::ALL),
        ),
        list,
    );

    let lines =
        state
            .snapshot
            .features
            .get(selected)
            .map(|feature| {
                let mut lines = vec![
                    field("ID", &feature.id),
                    field("Description", &feature.description),
                    field("Required gate", feature.required),
                    field("Status", &feature.status),
                    field("Coverage", &feature.coverage),
                    field("Scopes", feature.scopes.join(", ")),
                    field("Required proofs", feature.requirements),
                    field(
                        "Surface effects",
                        format!(
                            "{}/{} covered",
                            feature.covered_effects, feature.surface_effects
                        ),
                    ),
                    Line::from(""),
                ];
                if feature.blockers.is_empty() {
                    lines.push(Line::from("No qualification blockers"));
                } else {
                    lines.extend(
                        feature.blockers.iter().enumerate().map(|(index, blocker)| {
                            Line::from(format!("{}. {blocker}", index + 1))
                        }),
                    );
                }
                lines.push(Line::from(""));
                lines.push(Line::from("Lifecycle"));
                lines.extend(feature.phases.iter().map(|phase| {
                    Line::from(format!(
                        "{}: {}/{} transactions, {} proof(s), {} blocker(s)",
                        phase.id,
                        phase.covered_transactions,
                        phase.transactions,
                        phase.requirements,
                        phase.blockers,
                    ))
                }));
                if let Some(hardware) = &feature.hardware {
                    lines.push(Line::from(""));
                    lines.push(field("Hardware", &hardware.status));
                    lines.push(field(
                        "Successful runs",
                        format!(
                            "{}/{}",
                            hardware.successful_runs, hardware.minimum_successful_runs
                        ),
                    ));
                    lines.extend(
                        hardware
                            .blockers
                            .iter()
                            .map(|blocker| Line::from(format!("hardware: {blocker}"))),
                    );
                }
                lines
            })
            .unwrap_or_else(|| vec![Line::from("No configured feature qualifications")]);
    frame.render_widget(
        detail_paragraph(" Feature blockers ", lines, state.detail_scroll()),
        detail,
    );
}
