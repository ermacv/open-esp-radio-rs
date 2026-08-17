//! Generated evidence readiness without regenerating project outputs.

use std::collections::BTreeSet;

use super::model::{Component, LinkedIrProfileDetail, Phase, Readiness};
use crate::application::ProjectContext;
use crate::{artifacts::inspect_linked_ir, harnesses, run_spec::InputRole};

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    fn component(name: &'static str, collect: impl FnOnce() -> Component) -> Component {
        let started = std::time::Instant::now();
        let component = collect();
        tracing::debug!(
            component = name,
            elapsed_ms = started.elapsed().as_millis(),
            "analysis status component collected"
        );
        component
    }
    Phase::collect(
        "analysis",
        vec![
            component("symbol_inventory", || symbol_inventory(context)),
            component("linked_ir", || linked_ir(context)),
            component("event_replays", || event_replays(context)),
            component("mmio_facts", || mmio_facts(context)),
            component("interface_facts", || interface_facts(context)),
            component("navigation_index", || navigation_index(context)),
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
    generated_output("navigation_index", &spec.output)
}

fn project_command(context: &ProjectContext<'_>, command: &str) -> String {
    format!(
        "blobray {command} --project {}",
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
    generated_output("symbol_inventory", &spec.output)
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
            context.target.knowledge_provider.as_deref(),
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
    generated_output("mmio_facts", &paths.facts)
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
    generated_output("interface_facts", &paths.facts)
}

fn generated_output(name: &'static str, path: &std::path::Path) -> Component {
    match std::fs::metadata(path) {
        Ok(metadata) if metadata.is_file() && metadata.len() != 0 => {
            Component::new(name, Readiness::Ready)
                .detail("path", path.display().to_string())
                .detail("bytes", metadata.len())
                .detail("deep_validation", "project doctor / project check")
        }
        Ok(_) => Component::new(name, Readiness::Invalid)
            .detail("path", path.display().to_string())
            .diagnostic("generated output is not a non-empty regular file"),
        Err(error) => Component::new(name, Readiness::Invalid)
            .detail("path", path.display().to_string())
            .diagnostic(error),
    }
}
