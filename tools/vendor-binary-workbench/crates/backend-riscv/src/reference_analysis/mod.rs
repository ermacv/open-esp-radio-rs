//! Reference-trace resolution facade.

mod flatten;
mod flow;
mod inline;
mod resolver;
use flatten::flatten_reference_trace;
use flow::{
    ReferenceCalleeContext, compose_calls_in_reference_flow, explore_reference_flow,
    resolve_reference_callee, trace_into_reference_flow,
};
pub use inline::inline_reference_summary;
pub use resolver::{ReferenceResolver, ReferenceSymbolKey};

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

use super::static_analysis::{
    StructuralCallSite, StructuralPointerContext, StructuralRelocatedCalls, StructuralTraceBudget,
    SymbolicStack, is_reference_only_blocker, trace_binary_symbol_bounded,
    trace_binary_symbol_with_branches_bounded,
};
use crate::{
    DEFERRED_CALLER_MEMORY_REGION, DraftReferenceEvent, DraftReferenceFlow,
    DraftReferenceTerminator, FunctionAnalysis, IndexedMmioGuard, MemoryAccess, MmioMap,
    ObservableEvent, RV32_MODELED_ARGUMENT_COUNT, RV32_REGISTER_ARGUMENT_COUNT,
    RV32_STACK_ARGUMENT_COUNT, Result, Rv32CallArguments, SECONDARY_CALL_RESULT_TOKEN_FLAG,
    SymbolicValue, artifact, execution, reference_event_is_mmio_read,
    reference_flow_calls_are_valid,
};

pub fn resolve_reference_trace(
    symbol: &artifact::ArtifactSymbolDefinition,
    symbols_by_address: &BTreeMap<u32, artifact::ArtifactSymbolDefinition>,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    specialized_arguments: Option<&Rv32CallArguments>,
    svd: &MmioMap,
    visiting: &mut BTreeSet<u32>,
) -> Result<FunctionAnalysis> {
    resolve_reference_trace_with_budget(
        symbol,
        symbols_by_address,
        relocated_calls,
        pointer_context,
        specialized_arguments,
        svd,
        visiting,
        StructuralTraceBudget::UNBOUNDED,
    )
}

fn resolve_reference_trace_with_budget(
    symbol: &artifact::ArtifactSymbolDefinition,
    symbols_by_address: &BTreeMap<u32, artifact::ArtifactSymbolDefinition>,
    relocated_calls: &StructuralRelocatedCalls,
    pointer_context: &StructuralPointerContext,
    specialized_arguments: Option<&Rv32CallArguments>,
    svd: &MmioMap,
    visiting: &mut BTreeSet<u32>,
    budget: StructuralTraceBudget,
) -> Result<FunctionAnalysis> {
    if let Some(mut trace) = pointer_context
        .summary_hooks
        .and_then(|hooks| (hooks.reference_intrinsic)(symbol, svd, pointer_context))
    {
        if let Some(flow) = trace.reference_flow.take() {
            let original_flow = flow.clone();
            match compose_calls_in_reference_flow(
                flow,
                &ReferenceCalleeContext {
                    symbols_by_address,
                    relocated_calls,
                    pointer_context,
                    svd,
                    budget,
                },
                visiting,
                &mut trace.reference_dependencies,
            ) {
                Ok(flow) if reference_flow_calls_are_valid(&flow) => {
                    trace.reference_flow = Some(flow);
                }
                Ok(flow) => {
                    trace.reference_flow = Some(flow);
                    trace.reference_blockers.push(
                        "reviewed-summary: composed call result is used without a modeled callee `a0`"
                            .to_owned(),
                    );
                }
                Err(error) => {
                    trace.reference_flow = Some(original_flow);
                    trace
                        .reference_blockers
                        .push(format!("reviewed-summary: {error}"));
                }
            }
        }
        return Ok(trace);
    }
    let mut trace = trace_binary_symbol_bounded(
        symbol,
        svd,
        relocated_calls,
        pointer_context,
        specialized_arguments,
        budget,
    )?;
    trace
        .blockers
        .retain(|blocker| !is_reference_only_blocker(blocker));
    let typed_calls = trace
        .reference_events
        .iter()
        .filter(|event| {
            matches!(
                event,
                DraftReferenceEvent::TailCall { .. }
                    | DraftReferenceEvent::Call { .. }
                    | DraftReferenceEvent::DiagnosticCall { .. }
            )
        })
        .count();
    let has_private_stack_events = trace.reference_events.iter().any(|event| {
        matches!(
            event,
            DraftReferenceEvent::PrivateStackLoad { .. }
                | DraftReferenceEvent::PrivateStackStore { .. }
        )
    });
    if trace.unresolved_branch.is_some() {
        match explore_reference_flow(
            symbol,
            svd,
            relocated_calls,
            pointer_context,
            specialized_arguments,
            budget,
        )
        .and_then(|explored| {
            compose_calls_in_reference_flow(
                explored.flow,
                &ReferenceCalleeContext {
                    symbols_by_address,
                    relocated_calls,
                    pointer_context,
                    svd,
                    budget,
                },
                visiting,
                &mut trace.reference_dependencies,
            )
            .map(|flow| (flow, explored.incomplete_effects))
        }) {
            Ok((flow, incomplete_effects)) if reference_flow_calls_are_valid(&flow) => {
                trace.events.clear();
                trace.reference_events.clear();
                trace.blockers.clear();
                trace.reference_blockers = incomplete_effects;
                trace.reference_flow = Some(flow);
                trace.unresolved_branch = None;
            }
            Ok((flow, mut incomplete_effects)) => {
                trace.events.clear();
                trace.reference_events.clear();
                trace.blockers.clear();
                incomplete_effects.push(
                    "symbolic-cfg: composed call result is used without a modeled callee `a0`"
                        .to_owned(),
                );
                trace.reference_blockers = incomplete_effects;
                trace.reference_flow = Some(flow);
                trace.unresolved_branch = None;
            }
            Err(error) => trace
                .reference_blockers
                .push(format!("symbolic-cfg: {error}")),
        }
        return Ok(trace);
    }
    if typed_calls == 0 && !has_private_stack_events {
        return Ok(trace);
    }

    let call_blockers = trace
        .blockers
        .iter()
        .filter(|blocker| blocker.starts_with("call/jump instruction"))
        .count();
    if typed_calls != call_blockers {
        trace.reference_blockers.push(format!(
            "unsupported-call-shape: typed-calls={typed_calls} call-blockers={call_blockers}"
        ));
        return Ok(trace);
    }

    flatten_reference_trace(
        trace,
        symbols_by_address,
        relocated_calls,
        pointer_context,
        specialized_arguments,
        svd,
        visiting,
        budget,
    )
}
