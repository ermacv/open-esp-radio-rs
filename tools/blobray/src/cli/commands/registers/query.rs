//! Read-only catalog, coverage and evidence queries shared with the TUI.

use super::*;
use serde_json::json;

pub(super) fn run(
    command: RegisterWorkspaceCommand,
    session: &crate::application::ProjectSession,
) -> Result<bool> {
    let snapshot = &session.register_query()?.snapshot;
    let inventory = snapshot.inventory();
    let coverage = matches!(command, RegisterWorkspaceCommand::Coverage(_));
    match command {
        RegisterWorkspaceCommand::List(arguments)
        | RegisterWorkspaceCommand::Coverage(arguments) => {
            if arguments.limit == 0 {
                return Err(crate::Error::invalid("--limit must be positive"));
            }
            let address = |value: Option<&str>| {
                value
                    .map(|value| {
                        value
                            .strip_prefix("0x")
                            .map_or_else(
                                || value.parse::<u64>(),
                                |value| u64::from_str_radix(value, 16),
                            )
                            .map_err(|_| {
                                crate::Error::invalid(format!("invalid integer {value:?}"))
                            })
                    })
                    .transpose()
            };
            let query = crate::InventoryQuery {
                function: arguments.function.clone(),
                source: arguments.source.clone(),
                text: arguments.text.clone(),
                subject: arguments.subject.clone(),
                start: address(arguments.start.as_deref())?,
                end_exclusive: address(arguments.end_exclusive.as_deref())?,
                mask: address(arguments.mask.as_deref())?
                    .map(u32::try_from)
                    .transpose()
                    .map_err(|_| {
                        crate::Error::invalid("--mask exceeds the supported 32-bit mask")
                    })?,
                unknown: arguments.unknown,
                conflicted: arguments.conflicted,
            };
            let rows = inventory.select(&query);
            let total = rows.len();
            let selected = rows
                .into_iter()
                .skip(arguments.offset)
                .take(arguments.limit)
                .collect::<Vec<_>>();
            let next = arguments.offset.saturating_add(selected.len());
            let questions = inventory
                .questions()
                .into_iter()
                .filter(|question| {
                    selected
                        .iter()
                        .any(|register| register.id == question.subject)
                })
                .collect::<Vec<_>>();
            let document = json!({"schema_version":2,"snapshot_id":snapshot.id(),"command":if coverage { "registers coverage" } else { "registers list" },"total":total,"next_offset":(next < total).then_some(next),"registers":selected,"questions":questions,"sources":inventory.sources,"coverage_gaps":inventory.gaps,"regions":if coverage { Some(&inventory.regions) } else { None },"address_domains":inventory.address_domains});
            crate::cli::output::render_report(&document, || {
                outputln!(
                    "Registers: {} of {total}; offset={}",
                    selected.len(),
                    arguments.offset
                );
                for register in &selected {
                    outputln!(
                        "{:#010x} {} physical-width={} access-widths={:?} semantics={}\n  {}",
                        register.subject.address,
                        register.label(),
                        register
                            .width()
                            .map_or_else(|| "unknown".to_owned(), |width| width.to_string()),
                        register.access_widths,
                        if register.semantics.is_unknown() {
                            "unknown"
                        } else {
                            "asserted"
                        },
                        register.id
                    );
                }
                if next < total {
                    outputln!("More: --offset {next} --limit {}", arguments.limit);
                }
                outputln!(
                    "Coverage gaps: {} (use registers coverage)",
                    inventory.gaps.len()
                );
                outputln!(
                    "Address domains: {} (use registers evidence <ID>)",
                    inventory.address_domains.len()
                );
                if coverage {
                    for region in &inventory.regions {
                        outputln!(
                            "{} {:#x}..{:#x} boundary={}",
                            region.name,
                            region.start,
                            region.end_exclusive,
                            region.boundary
                        );
                        for (start, end) in &region.geometry_gaps {
                            outputln!("  unknown geometry {start:#x}..{end:#x}");
                        }
                        for (start, end) in &region.observation_gaps {
                            outputln!("  unobserved-in-scope {start:#x}..{end:#x}");
                        }
                    }
                    for register in &selected {
                        outputln!("{} coverage={:?}", register.id, register.coverage);
                    }
                    for gap in &inventory.gaps {
                        outputln!("INCOMPLETE {}: {} ({})", gap.scope, gap.reason, gap.source);
                    }
                }
            });
        }
        RegisterWorkspaceCommand::Evidence(arguments) => {
            let evidence = inventory.evidence.get(&arguments.id).ok_or_else(|| {
                crate::Error::invalid(format!(
                    "evidence {} is absent from the current sources",
                    arguments.id
                ))
            })?;
            crate::cli::output::render_report(evidence, || {
                outputln!(
                    "{}",
                    serde_json::to_string_pretty(evidence).expect("evidence serializes")
                )
            });
        }
        _ => unreachable!("only register queries are dispatched here"),
    }
    Ok(true)
}
