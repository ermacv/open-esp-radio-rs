//! Register list and lazy evidence detail rendering.

use ratatui::{
    Frame,
    layout::{Constraint, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Row, Table},
};

use super::{columns, detail_paragraph, field, heading, selected_style, table_rows};
use crate::tui::state::BrowserState;

pub(super) fn render(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let rows = state
        .snapshot
        .registers
        .registers
        .iter()
        .enumerate()
        .filter(|(index, _)| state.is_visible(*index))
        .skip(state.viewport_start(table_rows(list)))
        .take(table_rows(list))
        .map(|(index, register)| {
            Row::new([format!("0x{:08x}", register.address), register.name.clone()])
                .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        Table::new(rows, [Constraint::Length(12), Constraint::Min(12)])
            .header(Row::new(["Address", "Name"]).style(heading()))
            .block(
                Block::default()
                    .title(format!(
                        " Register catalog ({}/{}) ",
                        state.visible_count(),
                        state.snapshot.registers.registers.len()
                    ))
                    .borders(Borders::ALL),
            ),
        list,
    );
    let report = &state.snapshot.registers;
    let mut lines = vec![
        field("Configured", report.configured),
        field("Ranges", report.ranges),
        field("Observed", report.observed),
        field("Reviewed", report.reviewed),
        field("Outside publication scope", report.ignored),
        field("Non-operational only", report.non_operational),
        field("Manual", report.manual),
        field("Unreviewed", report.unreviewed),
        field("Fields", report.fields),
    ];
    if let Some(model) = &report.model {
        lines.push(field("Model", model.display()));
    }
    if let Some(register) = report.registers.get(state.selected()) {
        lines.push(Line::from(""));
        if let Some(detail) = state.register_detail(register.address) {
            render_detail(&mut lines, detail);
        } else {
            lines.push(field(
                "Selected address",
                format!("0x{:08x}", register.address),
            ));
            lines.push(field("Selected name", &register.name));
            lines.push(Line::from("Loading register evidence..."));
        }
    }
    frame.render_widget(
        detail_paragraph(" Register workspace ", lines, state.detail_scroll()),
        detail,
    );
}

fn render_detail(lines: &mut Vec<Line<'_>>, detail: &crate::RegisterDetailSummary) {
    lines.push(field("Address", format!("0x{:08x}", detail.address)));
    lines.push(field("Name", &detail.name));
    lines.push(field("Name source", detail.name_source.label()));
    lines.push(field("Review", detail.review_status.label()));
    lines.push(field(
        "Width",
        detail
            .width
            .map_or_else(|| "unknown".to_owned(), |width| format!("{width} bits")),
    ));
    lines.push(field(
        "Accesses",
        format!(
            "reads={} writes={} RMW={}",
            detail.reads, detail.writes, detail.read_modify_writes
        ),
    ));
    if let Some(confidence) = &detail.review_confidence {
        lines.push(field("Confidence", confidence));
    }
    if !detail.functions.is_empty() {
        lines.push(field("Functions", detail.functions.join(", ")));
    }
    if !detail.read_sites.is_empty() || !detail.write_sites.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Instruction sites",
            Style::new().add_modifier(Modifier::BOLD),
        )));
        for site in &detail.read_sites {
            lines.push(Line::from(format!(
                "READ  {:#010x}  {}",
                site.pc, site.function
            )));
        }
        for site in &detail.write_sites {
            lines.push(Line::from(format!(
                "WRITE {:#010x}  {}",
                site.pc, site.function
            )));
        }
    }
    if !detail.semantic_operations.is_empty() {
        lines.push(field("Semantics", detail.semantic_operations.join(", ")));
    }
    if !detail.fields.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Field candidates",
            Style::new().add_modifier(Modifier::BOLD),
        )));
        for candidate in &detail.fields {
            lines.push(Line::from(format!(
                "bits {}..{} mask={:#010x} writes={} predicates={} polls={}",
                candidate.most_significant_bit,
                candidate.least_significant_bit,
                candidate.mask,
                candidate.write_shapes,
                candidate.predicate_shapes,
                candidate.poll_shapes,
            )));
            if !candidate.semantic_operations.is_empty() {
                lines.push(Line::from(format!(
                    "  semantics: {}",
                    candidate.semantic_operations.join(", ")
                )));
            }
            for predicate in &candidate.predicates {
                lines.push(Line::from(format!(
                    "  {} predicate in {}: {}{}",
                    if predicate.transitive {
                        "transitive"
                    } else {
                        "direct"
                    },
                    predicate.function,
                    predicate.condition,
                    predicate
                        .effective_operation
                        .as_ref()
                        .map_or_else(String::new, |operation| format!(" [{operation}]")),
                )));
            }
        }
    }
    if !detail.write_patterns.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Write patterns",
            Style::new().add_modifier(Modifier::BOLD),
        )));
        for pattern in &detail.write_patterns {
            lines.push(Line::from(format!(
                "count={} modified={:#010x} preserved={:#010x} dynamic={:#010x}",
                pattern.occurrences,
                pattern.modified_mask,
                pattern.preserved_mask,
                pattern.dynamic_mask,
            )));
        }
    }
}
