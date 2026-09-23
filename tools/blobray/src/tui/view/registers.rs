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
        .registers()
        .enumerate()
        .filter(|(index, _)| state.is_visible(*index))
        .skip(state.viewport_start(table_rows(list)))
        .take(table_rows(list))
        .map(|(index, register)| {
            Row::new([
                format!("0x{:08x}", register.subject.address),
                register.label(),
            ])
            .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        Table::new(rows, [Constraint::Length(18), Constraint::Min(12)])
            .header(Row::new(["Address", "Name"]).style(heading()))
            .block(
                Block::default()
                    .title(format!(
                        " Register catalog ({}/{}) ",
                        state.visible_count(),
                        state.snapshot.registers.register_count()
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
        field("Fields", report.fields),
    ];
    if let Some(publication) = report.publication {
        lines.push(field("Publication reviewed", publication.reviewed));
        lines.push(field("Outside publication scope", publication.ignored));
        lines.push(field("Non-operational only", publication.non_operational));
        lines.push(field("Manual", publication.manual));
        lines.push(field("Publication unreviewed", publication.unreviewed));
    } else {
        lines.push(field("Publication review", "unknown / not configured"));
    }
    if let Some(model) = &report.model {
        lines.push(field("Model", model.display()));
    }
    if let Some(register) = report.register_at(state.selected()) {
        lines.push(Line::from(""));
        if let Some(detail) = state.register_detail(&register.id) {
            render_detail(&mut lines, detail);
        } else {
            lines.push(field(
                "Selected address",
                format!("0x{:08x}", register.subject.address),
            ));
            lines.push(field("Subject", &register.id));
            lines.push(field("Selected name", register.label()));
            lines.push(Line::from("Loading register evidence..."));
        }
    }
    match &report.inventory {
        crate::RegisterInventoryState::Failed { reason } => {
            lines.push(field("Inventory failed", reason))
        }
        crate::RegisterInventoryState::Available { snapshot } => {
            let inventory = snapshot.inventory();
            lines.push(field("Snapshot", snapshot.id()));
            for source in &inventory.sources {
                lines.push(Line::from(format!(
                    "SOURCE {} {} {:?}",
                    source.kind, source.path, source.state
                )));
            }
            for gap in &inventory.gaps {
                lines.push(Line::from(format!(
                    "INCOMPLETE {}: {} ({})",
                    gap.scope, gap.reason, gap.source
                )));
            }
            for domain in &inventory.address_domains {
                lines.push(field("Indexed domain", domain));
                if let Some(evidence) = inventory.evidence.get(domain) {
                    lines.push(field("Domain evidence", evidence.payload.to_string()));
                }
            }
            for region in &inventory.regions {
                lines.push(Line::from(format!(
                    "RANGE {} {} {:#x}..{:#x} {}",
                    region.address_space,
                    region.name,
                    region.start,
                    region.end_exclusive,
                    region.boundary
                )));
                for (start, end) in &region.geometry_gaps {
                    lines.push(Line::from(format!("unknown geometry {start:#x}..{end:#x}")));
                }
                for (start, end) in &region.observation_gaps {
                    lines.push(Line::from(format!(
                        "unobserved-in-scope {start:#x}..{end:#x}"
                    )));
                }
            }
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
    lines.push(field("Review", detail.review_status.label()));
    for region in &detail.regions {
        lines.push(field(
            "Range",
            format!("{} / {}", region.address_space, region.name),
        ));
    }
    lines.push(field(
        "Publication",
        match detail.publication_debt {
            Some(true) => "blocking",
            Some(false) => "not blocking",
            None => "unknown",
        },
    ));
    if !detail.publication_scopes.is_empty() {
        lines.push(field(
            "Publication scopes",
            detail.publication_scopes.join(", "),
        ));
    }
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
    if let Some(classification) = &detail.review_classification {
        lines.push(field("Fact classification", classification));
    }
    if !detail.functions.is_empty() {
        lines.push(field("Functions", detail.functions.join(", ")));
    }
    if !detail.non_operational_functions.is_empty() {
        lines.push(field(
            "Non-operational users",
            detail.non_operational_functions.join(", "),
        ));
    }
    if !detail.related_functions.is_empty() {
        lines.push(field(
            "Related IR aliases",
            detail.related_functions.join(", "),
        ));
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
    for subject in &detail.subjects {
        lines.push(Line::from(format!(
            "{} semantics={:?}",
            subject.id, subject.semantics
        )));
        lines.push(Line::from(format!("Names: {:?}", subject.names)));
        lines.push(Line::from(format!("Coverage: {:?}", subject.coverage)));
    }
    for gap in &detail.coverage_gaps {
        lines.push(Line::from(format!(
            "INCOMPLETE {}: {}",
            gap.scope, gap.reason
        )));
    }
    if !detail.fields.is_empty() {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            "Declared fields, hypotheses and unknown spans",
            Style::new().add_modifier(Modifier::BOLD),
        )));
        for candidate in &detail.fields {
            lines.push(Line::from(format!(
                "{} {} name={} evidence={}",
                candidate.subject,
                candidate.field.kind,
                if candidate.field.names.is_unknown() {
                    "unknown".to_owned()
                } else {
                    candidate
                        .field
                        .names
                        .values()
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>()
                        .join(" | ")
                },
                candidate
                    .field
                    .evidence
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
            lines.push(Line::from(format!(
                "bits {}..{} mask={} writes={} predicates={} polls={}",
                candidate.field.offset + candidate.field.width - 1,
                candidate.field.offset,
                candidate
                    .field
                    .mask
                    .map_or_else(|| "unknown".to_owned(), |mask| format!("{mask:#010x}")),
                candidate.write_shapes,
                candidate.predicate_shapes,
                candidate.poll_shapes,
            )));
            lines.push(Line::from(format!(
                "field semantics={:?}",
                candidate.field.semantics
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
