//! Generated evidence readiness without regenerating project outputs.

use std::collections::BTreeSet;

use super::model::{Component, LinkedIrProfileDetail, Phase, Readiness};
use crate::application::ProjectContext;
use crate::{
    artifacts::{inspect_linked_ir, inspect_symbol_inventory},
    harnesses,
    interfaces::InterfaceFacts,
    registers::RegisterFacts,
    run_spec::InputRole,
};

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    Phase::collect(
        "analysis",
        vec![
            symbol_inventory(context),
            linked_ir(context),
            event_replays(context),
            mmio_facts(context),
            interface_facts(context),
            navigation_index(context),
        ],
    )
}

fn event_replays(context: &ProjectContext<'_>) -> Component {
    let Some(functions) = context.project.functions.as_ref() else {
        return Component::new("event_replays", Readiness::NotConfigured);
    };
    if !functions.pack.is_file() {
        return Component::new("event_replays", Readiness::NotConfigured)
            .detail("pack", functions.pack.display().to_string());
    }
    let pack = match crate::function_workspace::FunctionPack::load_reviewed(&functions.pack) {
        Ok(pack) => pack,
        Err(error) => {
            return Component::new("event_replays", Readiness::Invalid)
                .detail("pack", functions.pack.display().to_string())
                .diagnostic(error);
        }
    };
    let replays = pack
        .event_routes
        .iter()
        .filter_map(|route| {
            route
                .replay
                .as_ref()
                .map(|replay| (route.id.as_str(), replay))
        })
        .collect::<Vec<_>>();
    if replays.is_empty() {
        return Component::new("event_replays", Readiness::NotConfigured)
            .detail("pack", functions.pack.display().to_string());
    }
    let mut outputs = Vec::with_capacity(replays.len());
    let mut problems = Vec::new();
    for (route, replay) in &replays {
        if !replay.evidence.is_file() {
            problems.push(format!(
                "event route {route:?} replay evidence has not been generated: {}",
                replay.evidence.display()
            ));
            outputs.push(format!("{route}: missing"));
            continue;
        }
        let result = std::fs::read_to_string(&replay.evidence)
            .map_err(|error| error.to_string())
            .and_then(|input| {
                crate::artifacts::parse_replay_evidence(&input)
                    .map(|_| ())
                    .map_err(|error| error.to_string())
            });
        match result {
            Ok(()) => outputs.push(format!("{route}: ready")),
            Err(error) => {
                outputs.push(format!("{route}: invalid"));
                problems.push(format!("event route {route:?}: {error}"));
            }
        }
    }
    let incomplete = !problems.is_empty();
    let mut component = Component::new(
        "event_replays",
        if incomplete {
            Readiness::Incomplete
        } else {
            Readiness::Ready
        },
    )
    .detail("pack", functions.pack.display().to_string())
    .detail("count", replays.len())
    .detail("routes", outputs);
    for problem in problems {
        component = component.diagnostic(problem);
    }
    if incomplete {
        component = component.next_action(project_command(context, "project analyze"));
    }
    component
}

fn navigation_index(context: &ProjectContext<'_>) -> Component {
    let Some(spec) = &context.project.navigation_index else {
        return Component::new("navigation_index", Readiness::NotConfigured);
    };
    if !spec.output.is_file() {
        return Component::new("navigation_index", Readiness::Incomplete)
            .detail("path", spec.output.display().to_string())
            .diagnostic("navigation index has not been generated")
            .next_action(project_command(context, "project analyze"));
    }
    match crate::navigation::inspect_report(&spec.output) {
        Ok(summary) => Component::new("navigation_index", Readiness::Ready)
            .detail("path", spec.output.display().to_string())
            .detail("artifacts", summary.artifacts)
            .detail("symbols", summary.symbols)
            .detail("linked_ir_functions", summary.linked_ir_functions)
            .detail("interface_callers", summary.interface_callers)
            .detail("interface_roots", summary.interface_roots)
            .detail(
                "unmatched_interface_roots",
                summary.unmatched_interface_roots,
            ),
        Err(error) => Component::new("navigation_index", Readiness::Invalid)
            .detail("path", spec.output.display().to_string())
            .diagnostic(error)
            .next_action(project_command(context, "project analyze")),
    }
}

fn project_command(context: &ProjectContext<'_>, command: &str) -> String {
    format!(
        "vendor-binary-workbench {command} --project {}",
        context.project_path.display()
    )
}

fn symbol_inventory(context: &ProjectContext<'_>) -> Component {
    let Some(spec) = &context.project.symbol_inventory else {
        return Component::new("symbol_inventory", Readiness::NotConfigured);
    };
    if !spec.output.is_file() {
        return Component::new("symbol_inventory", Readiness::Incomplete)
            .detail("path", spec.output.display().to_string())
            .diagnostic("symbol inventory has not been generated");
    }
    match inspect_symbol_inventory(&spec.output) {
        Ok(summary) => Component::new("symbol_inventory", Readiness::Ready)
            .detail("path", spec.output.display().to_string())
            .detail("artifacts", summary.artifacts)
            .detail("symbol_facts", summary.symbol_facts)
            .detail("exported_definitions", summary.exported_definitions)
            .detail("undefined", summary.undefined)
            .detail("unresolved_or_associated", summary.unresolved_or_associated)
            .detail("executable_bytes", summary.executable_bytes)
            .detail("symbol_covered_bytes", summary.symbol_covered_bytes)
            .detail(
                "uncovered_executable_bytes",
                summary.uncovered_executable_bytes,
            )
            .detail(
                "named_zero_sized_code_symbols",
                summary.named_zero_sized_code_symbols,
            )
            .detail(
                "function_boundary_candidates",
                summary.function_boundary_candidates,
            )
            .detail("code_recovery_blockers", summary.code_recovery_blockers),
        Err(error) => Component::new("symbol_inventory", Readiness::Invalid)
            .detail("path", spec.output.display().to_string())
            .diagnostic(error),
    }
}

fn linked_ir(context: &ProjectContext<'_>) -> Component {
    if context.project.ir_profiles.is_empty() {
        return Component::new("linked_ir", Readiness::NotConfigured);
    }
    let sources = context
        .run_spec
        .into_iter()
        .flat_map(crate::run_spec::RunSpec::inputs)
        .filter_map(|input| match &input.role {
            InputRole::SourceArtifact(source) => Some(source.as_str()),
            _ => None,
        })
        .collect::<BTreeSet<_>>();
    let mut invalid = false;
    let mut incomplete = false;
    let mut profiles = Vec::new();
    for profile in &context.project.ir_profiles {
        let requested = if profile.sources.is_empty() {
            sources.clone()
        } else {
            profile.sources.iter().map(String::as_str).collect()
        };
        let missing = requested.difference(&sources).copied().collect::<Vec<_>>();
        if context.run_spec.is_none() || requested.is_empty() || !missing.is_empty() {
            incomplete = true;
        }
        let contract = harnesses::entry_contract_or_neutral(
            context.target.harness.as_deref(),
            &profile.entry_contract,
        );
        if contract.is_err() {
            invalid = true;
        }
        let (output_status, summary, output_error) = if !profile.output.is_dir() {
            incomplete = true;
            ("not-generated", None, None)
        } else {
            match inspect_linked_ir(&profile.output) {
                Ok(summary) => ("ready", Some(summary), None),
                Err(error) => {
                    invalid = true;
                    ("invalid", None, Some(error.to_string()))
                }
            }
        };
        let contract_status = if contract.is_ok() { "ready" } else { "invalid" };
        profiles.push(LinkedIrProfileDetail {
            id: profile.id.clone(),
            sources: requested
                .iter()
                .map(|source| (*source).to_owned())
                .collect(),
            missing_sources: missing.iter().map(|source| (*source).to_owned()).collect(),
            entry_contract: profile.entry_contract.clone(),
            contract_status,
            contract_error: contract.err().map(|error| error.to_string()),
            output: profile.output.display().to_string(),
            output_status,
            output_error,
            functions: summary.as_ref().map_or(0, |summary| summary.functions),
            registers: summary.as_ref().map_or(0, |summary| summary.registers),
            field_candidates: summary
                .as_ref()
                .map_or(0, |summary| summary.field_candidates),
        });
    }
    Component::new(
        "linked_ir",
        if invalid {
            Readiness::Invalid
        } else if incomplete {
            Readiness::Incomplete
        } else {
            Readiness::Ready
        },
    )
    .detail("profiles", profiles)
}

fn mmio_facts(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.registers else {
        return Component::new("mmio_facts", Readiness::NotConfigured);
    };
    if !paths.facts.is_file() {
        return Component::new("mmio_facts", Readiness::Incomplete)
            .detail("path", paths.facts.display().to_string())
            .diagnostic("MMIO facts have not been generated");
    }
    match RegisterFacts::load(&paths.facts) {
        Ok(facts) => Component::new("mmio_facts", Readiness::Ready)
            .detail("path", paths.facts.display().to_string())
            .detail("ranges", facts.ranges.len())
            .detail("registers", facts.registers.len()),
        Err(error) => Component::new("mmio_facts", Readiness::Invalid)
            .detail("path", paths.facts.display().to_string())
            .diagnostic(error),
    }
}

fn interface_facts(context: &ProjectContext<'_>) -> Component {
    let Some(paths) = &context.project.interfaces else {
        return Component::new("interface_facts", Readiness::NotConfigured);
    };
    if !paths.facts.is_file() {
        return Component::new("interface_facts", Readiness::Incomplete)
            .detail("path", paths.facts.display().to_string())
            .diagnostic("interface facts have not been generated");
    }
    match InterfaceFacts::load(&paths.facts) {
        Ok(facts) => Component::new("interface_facts", Readiness::Ready)
            .detail("path", paths.facts.display().to_string())
            .detail("tables", facts.tables.len())
            .detail("observed_slots", facts.observed_slots())
            .detail("observed_calls", facts.observed_calls()),
        Err(error) => Component::new("interface_facts", Readiness::Invalid)
            .detail("path", paths.facts.display().to_string())
            .diagnostic(error),
    }
}
