//! Direct typed operations used by CLI-independent frontends.

use super::{ProjectSession, model::*};
use crate::{ObservableEvent, analysis, interfaces::InterfaceWorkspace, verification::*};

pub(super) fn analyze(
    resolved: &ProjectSession,
    request: &AnalyzeRequest,
) -> crate::Result<AnalysisReport> {
    let trace = analysis::extract(
        &analysis::ArtifactSymbolSelector {
            artifact: request.artifact.clone(),
            member: request.member.clone(),
            symbol: request.symbol.clone(),
        },
        &resolved.mmio,
    )?;
    Ok(AnalysisReport {
        symbol: trace.symbol.clone(),
        exact: trace.is_exact(),
        reference_codegen_eligible: trace.is_reference_eligible(),
        return_value: trace.return_value.canonical(),
        events: trace.events.iter().map(|event| event.canonical()).collect(),
        blockers: trace.blockers.clone(),
        reference_blockers: trace.reference_blockers.clone(),
        unnamed_mmio: trace
            .events
            .iter()
            .filter_map(ObservableEvent::unmapped_address)
            .collect(),
    })
}

pub(super) fn compare(
    resolved: &ProjectSession,
    request: CompareRequest,
) -> crate::Result<ExecutionComparisonReport> {
    validate_table_instances(resolved, &request.scenarios)?;
    let scenarios = if request.scenarios.is_empty() {
        vec![NamedScenario::new("default".to_owned())]
    } else {
        request
            .scenarios
            .into_iter()
            .map(|scenario| {
                let mut named = NamedScenario::new(scenario.name);
                named.scenario = scenario.scenario;
                named.vendor_table_instances = scenario.vendor_table_instances;
                named.rust_table_instances = scenario.rust_table_instances;
                named.vendor_fifo_services = scenario.vendor_fifo_services;
                named.rust_fifo_services = scenario.rust_fifo_services;
                named.vendor_fifo_bindings = scenario.vendor_fifo_bindings;
                named.rust_fifo_bindings = scenario.rust_fifo_bindings;
                named.vendor_goal = scenario.vendor_goal;
                named.rust_goal = scenario.rust_goal;
                named
            })
            .collect()
    };
    let unconstrained = [[None; 8]];
    let argument_domain: &[[Option<u32>; 8]] = if request.argument_domain.is_empty() {
        &unconstrained[..]
    } else {
        request.argument_domain.as_slice()
    };
    let coverage_domain = argument_domain
        .iter()
        .copied()
        .map(
            |arguments| crate::verification::profiles::ProfileCoverageConstraint {
                arguments,
                stable_words: std::collections::BTreeMap::new(),
            },
        )
        .collect::<Vec<_>>();
    compare_execution_scenarios(
        &resolved.mmio,
        ExecutionInput {
            artifact: &request.vendor_artifact,
            companion: request.vendor_companion.as_deref(),
            symbol: &request.vendor_symbol,
        },
        ExecutionInput {
            artifact: &request.rust_artifact,
            companion: request.rust_companion.as_deref(),
            symbol: &request.rust_symbol,
        },
        crate::ExecutionComparisonPolicy {
            compare_return: request.compare_return,
            case_execution: crate::verification::profiles::CaseExecution::Independent,
            transaction_comparison:
                crate::verification::profiles::TransactionComparison::Observables,
            effect_policy: None,
            call_equivalences: &[],
            diagnostic_contracts: crate::providers::diagnostic_contracts_or_empty(
                resolved.target.knowledge_provider.as_deref(),
            )?,
            coverage_domain: &coverage_domain,
            vendor_setup: &[],
        },
        &scenarios,
    )
}

pub(super) fn validate_table_instances(
    resolved: &ProjectSession,
    scenarios: &[ComparisonScenario],
) -> crate::Result<()> {
    let instances = scenarios.iter().flat_map(|scenario| {
        scenario
            .scenario
            .table_instances
            .iter()
            .chain(&scenario.vendor_table_instances)
            .chain(&scenario.rust_table_instances)
    });
    let instances = instances.collect::<Vec<_>>();
    if instances.is_empty() {
        return Ok(());
    }
    let paths = resolved.project.interfaces.as_ref().ok_or_else(|| {
        crate::Error::invalid(
            "application comparison with runtime tables requires configured interface facts and pack",
        )
    })?;
    let pack = paths.pack.as_ref().ok_or_else(|| {
        crate::Error::invalid(
            "application comparison with runtime tables requires a reviewed interface pack",
        )
    })?;
    let harness = resolved
        .target
        .knowledge_provider
        .as_deref()
        .and_then(|harness| crate::providers::contracts(harness).ok());
    let workspace = InterfaceWorkspace::load_with_templates(
        &paths.facts,
        pack,
        &paths.semantic_catalogs,
        &paths.interface_template_packs,
        resolved.target.calling_convention.label(),
        harness,
    )?;
    for instance in instances {
        workspace.validate_table_instance(instance)?;
    }
    Ok(())
}
