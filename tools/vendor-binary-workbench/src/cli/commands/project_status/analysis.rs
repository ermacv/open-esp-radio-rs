//! Generated evidence readiness without regenerating project outputs.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use super::{
    super::ProjectContext,
    model::{Component, Phase, Readiness},
};
use crate::{
    harnesses, interfaces::InterfaceFacts, project_ir_report::inspect_project_ir_report,
    registers::RegisterFacts,
};

pub(super) fn collect(context: &ProjectContext<'_>) -> Phase {
    Phase::collect(
        "analysis",
        vec![
            linked_ir(context),
            mmio_facts(context),
            interface_facts(context),
        ],
    )
}

fn linked_ir(context: &ProjectContext<'_>) -> Component {
    if context.project.ir_profiles.is_empty() {
        return Component::new("linked_ir", Readiness::NotConfigured);
    }
    let sources = context
        .run_spec
        .into_iter()
        .flat_map(crate::run_spec::RunSpec::inputs)
        .filter_map(|(role, _)| role.strip_prefix("source-artifact:"))
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
        let (output_status, summary, output_error) = if !profile.output.is_file() {
            incomplete = true;
            ("not-generated", None, None)
        } else {
            match inspect_project_ir_report(&profile.output) {
                Ok(summary) => ("ready", Some(summary), None),
                Err(error) => {
                    invalid = true;
                    ("invalid", None, Some(error.to_string()))
                }
            }
        };
        let pseudo_status = match profile.pseudo_rust.as_deref() {
            None => "not-configured",
            Some(path) if path.is_file() => "ready",
            Some(_) => {
                incomplete = true;
                "not-generated"
            }
        };
        profiles.push(json!({
            "id": profile.id,
            "sources": requested,
            "missing_sources": missing,
            "entry_contract": profile.entry_contract,
            "contract_status": if contract.is_ok() { "ready" } else { "invalid" },
            "contract_error": contract.err().map(|error| error.to_string()),
            "output": profile.output.display().to_string(),
            "output_status": output_status,
            "output_error": output_error,
            "functions": summary.as_ref().map_or(0, |summary| summary.functions),
            "registers": summary.as_ref().map_or(0, |summary| summary.registers),
            "field_candidates": summary.as_ref().map_or(0, |summary| summary.field_candidates),
            "pseudo_rust": profile.pseudo_rust.as_deref().map(|path| path.display().to_string()),
            "pseudo_status": pseudo_status,
        }));
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
    .detail("profiles", Value::Array(profiles))
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
