//! Function index, reviewed metadata, scenario candidates and pseudo-code view.

use ratatui::{
    Frame,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem},
};

use super::super::state::BrowserState;
use super::{columns, detail_paragraph, field, list_rows, selected_style};

pub(super) fn render(frame: &mut Frame<'_>, state: &BrowserState, area: Rect) {
    let [list, detail] = columns(area);
    let items = state
        .snapshot
        .functions
        .iter()
        .enumerate()
        .filter(|(index, _)| state.is_visible(*index))
        .skip(state.viewport_start(list_rows(list)))
        .take(list_rows(list))
        .map(|(index, function)| {
            ListItem::new(Line::from(vec![
                Span::raw(&function.symbol),
                Span::raw("  "),
                Span::styled(
                    function.review_status.label(),
                    Style::new().fg(if function.complete {
                        Color::Green
                    } else {
                        Color::Yellow
                    }),
                ),
                Span::raw(if function.decode_blockers == 0 {
                    String::new()
                } else {
                    format!("  decode:{}", function.decode_blockers)
                }),
            ]))
            .style(selected_style(index, state.selected()))
        });
    frame.render_widget(
        List::new(items).block(
            Block::default()
                .title(format!(
                    " Functions ({}/{}) ",
                    state.visible_count(),
                    state.snapshot.functions.len()
                ))
                .borders(Borders::ALL),
        ),
        list,
    );

    let lines = state
        .snapshot
        .functions
        .get(state.selected())
        .map(|function| {
            let function_detail = state.function_detail(&function.identity);
            let mut lines = vec![
                field("Identity", &function.identity),
                field("Source", &function.source),
                field("Profile", &function.profile),
                field("Selection", function.selection.label()),
                field("Calls", function.calls.to_string()),
                field("Decode blockers", function.decode_blockers),
                field("Registers", function.registers.len()),
                field(
                    "Contexts",
                    function_detail.map_or(0, |detail| detail.contexts.len()),
                ),
                field(
                    "Memory fields",
                    function_detail.map_or(0, |detail| detail.memory_fields.len()),
                ),
            ];
            if let Some(role) = &function.role {
                lines.push(field("Role", role));
            }
            if let Some(summary) = &function.summary {
                lines.push(Line::from(""));
                lines.push(Line::from(summary.clone()));
            }
            if !function.blockers.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Blockers",
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )));
                lines.extend(
                    function
                        .blockers
                        .iter()
                        .map(|blocker| Line::from(format!("- {blocker}"))),
                );
            }
            if !function.mmio_sites.is_empty() {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Artifact-wide MMIO sites",
                    Style::new().add_modifier(Modifier::BOLD),
                )));
                lines.extend(function.mmio_sites.iter().map(|site| {
                    Line::from(format!(
                        "- {:5} {:#010x}/{} @ {:#010x}",
                        site.access.to_ascii_uppercase(),
                        site.address,
                        site.width,
                        site.pc,
                    ))
                }));
            }
            if let Some(detail) =
                function_detail.filter(|detail| !detail.decode_blockers.is_empty())
            {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Decode blockers",
                    Style::new().fg(Color::Yellow).add_modifier(Modifier::BOLD),
                )));
                lines.extend(detail.decode_blockers.iter().map(|blocker| {
                    Line::from(format!(
                        "- {:#010x}: {} ({}) width={} raw={:#010x} flow={}",
                        blocker.address,
                        blocker.operation,
                        blocker.class,
                        blocker.width,
                        blocker.raw,
                        if blocker.linear_control_flow {
                            "linear"
                        } else {
                            "blocked"
                        }
                    ))
                }));
            }
            if let Some(detail) = function_detail.filter(|detail| !detail.memory_fields.is_empty())
            {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Memory objects",
                    Style::new().add_modifier(Modifier::BOLD),
                )));
                lines.extend(detail.memory_fields.iter().map(|field| {
                    Line::from(format!(
                        "- {} {:+#x}/{} read={} write={} mask={:#010x}",
                        field.object,
                        field.offset,
                        field.width,
                        field.reads,
                        field.writes,
                        field.write_mask
                    ))
                }));
            }
            if let Some(detail) =
                function_detail.filter(|detail| !detail.scenario_suggestions.is_empty())
            {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Scenario candidates (replay required)",
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                )));
                for suggestion in &detail.scenario_suggestions {
                    lines.push(Line::from(format!(
                        "- {} @ {}: {}",
                        suggestion.kind,
                        suggestion
                            .site
                            .map_or_else(|| "-".to_owned(), |site| format!("{site:#010x}")),
                        suggestion.evidence
                    )));
                    for variant in &suggestion.variants {
                        let arguments = variant
                            .arguments
                            .iter()
                            .map(|argument| format!("a{}={:#010x}", argument.index, argument.value))
                            .collect::<Vec<_>>()
                            .join(", ");
                        let reads = variant
                            .mmio_reads
                            .iter()
                            .map(|read| {
                                format!(
                                    "read {:#010x}=[{}]",
                                    read.address,
                                    read.values
                                        .iter()
                                        .map(|value| format!("{value:#010x}"))
                                        .collect::<Vec<_>>()
                                        .join(", ")
                                )
                            })
                            .collect::<Vec<_>>()
                            .join("; ");
                        lines.push(Line::from(format!(
                            "    {}  {}{}{}",
                            variant.name,
                            arguments,
                            if arguments.is_empty() || reads.is_empty() {
                                ""
                            } else {
                                "; "
                            },
                            reads
                        )));
                    }
                }
            }
            if let Some(draft) = function_detail.and_then(|detail| detail.profile_draft.as_ref()) {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Editable profile draft (replay required)",
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                )));
                lines.extend(draft.lines().map(|line| Line::from(line.to_owned())));
            }
            if let Some(detail) = function_detail.filter(|detail| !detail.pseudo_rust.is_empty()) {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Pseudo-Rust",
                    Style::new().add_modifier(Modifier::BOLD),
                )));
                lines.extend(
                    detail
                        .pseudo_rust
                        .lines()
                        .map(|line| Line::from(line.to_owned())),
                );
            }
            if let Some(detail) = function_detail.filter(|detail| {
                !detail.reviewed_preconditions.is_empty() || !detail.reviewed_paths.is_empty()
            }) {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Reviewed path knowledge (assumptions, not execution proof)",
                    Style::new().fg(Color::Magenta).add_modifier(Modifier::BOLD),
                )));
                lines.extend(detail.reviewed_preconditions.iter().map(|precondition| {
                    Line::from(format!(
                        "- precondition {}: {} — {}",
                        precondition.id, precondition.expression, precondition.rationale
                    ))
                }));
                lines.extend(detail.reviewed_paths.iter().map(|path| {
                    Line::from(format!(
                        "- path {} [{}]: {} ({})",
                        path.id, path.class, path.summary, path.evidence
                    ))
                }));
            }
            if let Some(investigation) =
                function_detail.and_then(|detail| detail.investigation.as_ref())
            {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    "Lossless investigation",
                    Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
                )));
                lines.extend(investigation.proof_ledger.iter().map(|entry| {
                    Line::from(format!(
                        "- {:<12} {:<20} {}",
                        entry.layer, entry.status, entry.detail
                    ))
                }));
                for semantic in &investigation.semantics {
                    if !semantic.calls.is_empty() {
                        lines.push(Line::from(""));
                        lines.push(Line::from(Span::styled(
                            format!("Call boundaries ({})", semantic.profile),
                            Style::new().add_modifier(Modifier::BOLD),
                        )));
                        lines.extend(semantic.calls.iter().map(|call| {
                            Line::from(format!(
                                "- {} {}{} [{}]{}{}",
                                call.kind,
                                call.target,
                                call.site
                                    .map(|site| format!(" @ {site:#010x}"))
                                    .unwrap_or_default(),
                                call.knowledge,
                                call.semantic_operation
                                    .as_deref()
                                    .map(|operation| format!(" operation={operation}"))
                                    .unwrap_or_default(),
                                call.execution_model
                                    .as_deref()
                                    .map(|model| format!(" model={model}"))
                                    .unwrap_or_default(),
                            ))
                        }));
                    }
                    if !semantic.call_graph_edges.is_empty() {
                        lines.push(Line::from(format!(
                            "Reachable call graph: {} functions, {} edges",
                            semantic.reachable_functions.len(),
                            semantic.call_graph_edges.len()
                        )));
                        lines.extend(semantic.call_graph_edges.iter().take(100).map(|edge| {
                            Line::from(format!(
                                "  {} --{}{}--> {}",
                                edge.caller,
                                edge.kind,
                                edge.site
                                    .map(|site| format!("@{site:#010x}"))
                                    .unwrap_or_default(),
                                edge.callee
                            ))
                        }));
                        if semantic.call_graph_edges.len() > 100 {
                            lines.push(Line::from(format!(
                                "  ... {} additional edges",
                                semantic.call_graph_edges.len() - 100
                            )));
                        }
                    }
                    if !semantic.event_dispatches.is_empty() {
                        lines.push(Line::from("Event dispatches:"));
                        for dispatch in &semantic.event_dispatches {
                            let bindings = dispatch
                                .bindings
                                .iter()
                                .map(|binding| format!("{}={}", binding.role, binding.value))
                                .collect::<Vec<_>>()
                                .join(", ");
                            lines.push(Line::from(format!(
                                "  {} -> {} ({}, [{}])",
                                dispatch.mechanism,
                                dispatch.receiver.as_deref().unwrap_or("unknown receiver"),
                                dispatch.execution_context,
                                bindings
                            )));
                            lines.extend(
                                dispatch
                                    .blockers
                                    .iter()
                                    .map(|blocker| Line::from(format!("    ! {blocker}"))),
                            );
                        }
                    }
                    if !semantic.reviewed_event_routes.is_empty() {
                        lines.push(Line::from("Reviewed event routes:"));
                        for route in &semantic.reviewed_event_routes {
                            lines.push(Line::from(format!(
                                "  {}: {} {}={:#010x} -> {} [{}]",
                                route.id,
                                route.mechanism,
                                route.selector_role,
                                route.selector_value,
                                route.handler,
                                if route.dispatch_constraint_matched {
                                    "matched"
                                } else {
                                    "blocked"
                                }
                            )));
                            if let Some(handler) = &route.handler_analysis {
                                lines.push(Line::from(format!(
                                    "    handler complete={} effects={} calls={} reachable={}",
                                    handler.complete,
                                    handler.direct_instruction_effects,
                                    handler.direct_calls,
                                    handler.reachable_functions
                                )));
                                lines.extend(
                                    handler
                                        .blockers
                                        .iter()
                                        .map(|blocker| Line::from(format!("      ! {blocker}"))),
                                );
                            }
                            lines.extend(
                                route
                                    .blockers
                                    .iter()
                                    .map(|blocker| Line::from(format!("    ! {blocker}"))),
                            );
                        }
                    }
                }
                if !investigation.replacements.is_empty() {
                    lines.push(Line::from(""));
                    lines.push(Line::from(Span::styled(
                        "Vendor ↔ Rust replacement",
                        Style::new().add_modifier(Modifier::BOLD),
                    )));
                    lines.extend(investigation.replacements.iter().map(|replacement| {
                        Line::from(format!(
                            "{}:{} status={} production={} proofs={} (freshness not claimed)",
                            replacement.vendor_source,
                            replacement.vendor_symbol,
                            replacement.status,
                            replacement
                                .production_component
                                .as_deref()
                                .unwrap_or("none"),
                            replacement.proofs.as_array().map_or(0, Vec::len),
                        ))
                    }));
                }
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!("CFG ({} blocks)", investigation.runtime.basic_blocks.len()),
                    Style::new().add_modifier(Modifier::BOLD),
                )));
                lines.extend(investigation.runtime.basic_blocks.iter().map(|block| {
                    let successors = block
                        .successors
                        .iter()
                        .map(|successor| {
                            successor.block.map_or_else(
                                || format!("{} -> ?", successor.kind),
                                |target| format!("{} -> bb{target}", successor.kind),
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ");
                    Line::from(format!(
                        "- bb{} +{:#06x}..+{:#06x} {} [{}]",
                        block.id,
                        block.start_offset,
                        block.end_offset,
                        if block.reachable {
                            "reachable"
                        } else {
                            "unreachable"
                        },
                        successors
                    ))
                }));
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    format!(
                        "Full body ({}/{} bytes)",
                        investigation.runtime.accounted_bytes, investigation.runtime.size
                    ),
                    Style::new().add_modifier(Modifier::BOLD),
                )));
                for instruction in &investigation.runtime.instructions {
                    for label in investigation
                        .runtime
                        .labels
                        .iter()
                        .filter(|label| label.offset == instruction.offset)
                    {
                        lines.push(Line::from(Span::styled(
                            format!("{}:", label.name),
                            Style::new().add_modifier(Modifier::BOLD),
                        )));
                    }
                    lines.push(Line::from(Span::styled(
                        format!(
                            "  +{:#06x} {:<10} {:<28} {}",
                            instruction.offset,
                            instruction.raw,
                            instruction.text,
                            instruction.control_flow.kind.label()
                        ),
                        if instruction.supported {
                            Style::new()
                        } else {
                            Style::new().fg(Color::Yellow)
                        },
                    )));
                    for semantic in &investigation.semantics {
                        let Some(evidence) = semantic
                            .instruction_evidence
                            .iter()
                            .find(|evidence| evidence.address == instruction.address)
                        else {
                            continue;
                        };
                        lines.extend(evidence.effects.iter().map(|effect| {
                            let mut detail = format!(
                                "      = {} {}{} {}",
                                effect.kind, effect.access, effect.width, effect.target
                            );
                            if let Some(value) = &effect.value {
                                detail.push_str(" value=");
                                detail.push_str(value);
                            }
                            Line::from(Span::styled(detail, Style::new().fg(Color::Cyan)))
                        }));
                        lines.extend(effect_guard_lines(&evidence.effects));
                        if !evidence.call_targets.is_empty() {
                            lines.push(Line::from(format!(
                                "      = calls {}",
                                evidence.call_targets.join(", ")
                            )));
                        }
                        if !evidence.semantic_operations.is_empty() {
                            lines.push(Line::from(format!(
                                "      = semantics {}",
                                evidence.semantic_operations.join(", ")
                            )));
                        }
                        lines.extend(
                            evidence
                                .blocker_ids
                                .iter()
                                .map(|blocker| Line::from(format!("      ! {blocker}"))),
                        );
                    }
                    lines.extend(instruction.relocations.iter().map(|relocation| {
                        Line::from(format!(
                            "      @ {} {} {:+}",
                            relocation.kind, relocation.symbol, relocation.addend
                        ))
                    }));
                }
                if let Some(origin) = &investigation.origin {
                    lines.push(Line::from(""));
                    lines.push(Line::from(format!(
                        "Origin: {}{} ({}/{} bytes, {})",
                        origin.body.artifact,
                        origin
                            .body
                            .member
                            .as_deref()
                            .map(|member| format!("({member})"))
                            .unwrap_or_default(),
                        origin.body.accounted_bytes,
                        origin.body.size,
                        origin.association
                    )));
                }
            }
            lines
        })
        .unwrap_or_else(|| vec![Line::from("Function IR/review facts are not available")]);
    frame.render_widget(
        detail_paragraph(" Function detail ", lines, state.detail_scroll()),
        detail,
    );
}

fn effect_guard_lines(
    effects: &[crate::function_investigation::InstructionEffectEvidence],
) -> Vec<Line<'static>> {
    effects
        .iter()
        .filter(|effect| !effect.guards.is_empty())
        .map(|effect| Line::from(format!("        guards: {}", effect.guards.join("; "))))
        .collect()
}
