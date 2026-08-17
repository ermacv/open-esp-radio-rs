//! Reference-trace resolution facade.

mod flatten;
mod flow;
mod inline;
mod intrinsics;
mod resolver;
use flatten::flatten_reference_trace;
use flow::{
    ReferenceCalleeContext, compose_calls_in_reference_flow, explore_reference_flow,
    resolve_reference_callee, trace_into_reference_flow,
};
pub use inline::inline_reference_summary;
use intrinsics::standard_memory_intrinsic_trace;
pub use resolver::{ReferenceResolver, ReferenceSymbolKey};

use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet, VecDeque},
    path::{Path, PathBuf},
};

const MAX_MEMOIZED_CALLEE_VARIANTS: usize = 2_048;

/// Process-local immutable callee summaries shared by one analysis worker.
///
/// The exact target and complete RV32 symbolic argument array form the key.
/// Completed eligible and ineligible traces enter the memo; a
/// recursion-dependent failure cannot escape its original caller.
pub struct ReferenceAnalysisMemo {
    callee_cache: RefCell<BTreeMap<u32, Vec<(Rv32CallArguments, FunctionAnalysis)>>>,
    entries: Cell<usize>,
    hits: Cell<usize>,
}

impl Default for ReferenceAnalysisMemo {
    fn default() -> Self {
        Self {
            callee_cache: RefCell::new(BTreeMap::new()),
            entries: Cell::new(0),
            hits: Cell::new(0),
        }
    }
}

impl ReferenceAnalysisMemo {
    pub fn entries(&self) -> usize {
        self.entries.get()
    }

    pub fn hits(&self) -> usize {
        self.hits.get()
    }

    fn get(&self, target: u32, arguments: &Rv32CallArguments) -> Option<FunctionAnalysis> {
        let trace = self
            .callee_cache
            .borrow()
            .get(&target)
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|(cached_arguments, _)| cached_arguments == arguments)
            })
            .map(|(_, trace)| trace.clone())?;
        self.hits.set(self.hits.get() + 1);
        Some(trace)
    }

    fn insert_completed(
        &self,
        target: u32,
        arguments: &Rv32CallArguments,
        trace: &FunctionAnalysis,
        failure_reasons: Option<&[String]>,
    ) {
        let recursion_dependent = failure_reasons.is_some_and(|reasons| {
            reasons
                .iter()
                .any(|reason| reason.contains("recursive-call"))
        });
        if recursion_dependent || self.entries.get() >= MAX_MEMOIZED_CALLEE_VARIANTS {
            return;
        }
        let mut cache = self.callee_cache.borrow_mut();
        let entries = cache.entry(target).or_default();
        if !entries
            .iter()
            .any(|(cached_arguments, _)| cached_arguments == arguments)
        {
            entries.push((arguments.clone(), trace.clone()));
            self.entries.set(self.entries.get() + 1);
        }
    }
}

#[cfg(test)]
mod memo_tests {
    use super::*;

    fn arguments() -> Rv32CallArguments {
        std::array::from_fn(|index| SymbolicValue::Input { index: index as u8 })
    }

    fn trace(blocker: &str) -> FunctionAnalysis {
        FunctionAnalysis {
            symbol: "callee".to_owned(),
            events: Vec::new(),
            located_events: Vec::new(),
            located_reference_events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: Vec::new(),
            blockers: vec![blocker.to_owned()],
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Unknown,
            reference_flow: None,
            unresolved_branch: None,
        }
    }

    #[test]
    fn completed_ineligible_trace_is_exactly_keyed_but_recursion_is_not_cached() {
        let memo = ReferenceAnalysisMemo::default();
        let arguments = arguments();
        let ineligible = trace("unmodeled-memory-load at 0x1000");
        let reasons = ineligible.reference_failure_reasons();
        memo.insert_completed(0x2000, &arguments, &ineligible, Some(&reasons));

        assert_eq!(memo.entries(), 1);
        assert_eq!(memo.get(0x2000, &arguments), Some(ineligible));
        let mut different = arguments.clone();
        different[0] = SymbolicValue::Constant(1);
        assert!(memo.get(0x2000, &different).is_none());

        let recursive = trace("recursive-call at 0x3000 to callee");
        let reasons = recursive.reference_failure_reasons();
        memo.insert_completed(0x3000, &arguments, &recursive, Some(&reasons));
        assert!(memo.get(0x3000, &arguments).is_none());
    }
}

use super::static_analysis::{
    StructuralCallSite, StructuralPointerContext, StructuralRelocatedCalls, StructuralTraceBudget,
    SymbolicStack, is_reference_only_blocker, trace_binary_symbol_bounded,
    trace_binary_symbol_with_branches_bounded,
};
use crate::{
    DEFERRED_CALLER_MEMORY_REGION, DraftReferenceEvent, DraftReferenceFlow,
    DraftReferenceTerminator, FunctionAnalysis, IndexedMmioGuard, LocatedObservableEvent,
    LocatedReferenceEvent, MemoryAccess, MmioMap, ObservableEvent, RV32_MODELED_ARGUMENT_COUNT,
    RV32_REGISTER_ARGUMENT_COUNT, RV32_STACK_ARGUMENT_COUNT, Result, Rv32CallArguments,
    SECONDARY_CALL_RESULT_TOKEN_FLAG, SymbolicValue, artifact, execution,
    reference_event_is_mmio_read, reference_flow_call_validation_error,
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
    let memo = ReferenceAnalysisMemo::default();
    let context = ReferenceCalleeContext {
        symbols_by_address,
        relocated_calls,
        pointer_context,
        svd,
        budget: StructuralTraceBudget::UNBOUNDED,
        memo: &memo,
    };
    resolve_reference_trace_with_budget(symbol, &context, specialized_arguments, visiting)
}

fn resolve_reference_trace_with_budget(
    symbol: &artifact::ArtifactSymbolDefinition,
    context: &ReferenceCalleeContext<'_>,
    specialized_arguments: Option<&Rv32CallArguments>,
    visiting: &mut BTreeSet<u32>,
) -> Result<FunctionAnalysis> {
    if let Some(mut trace) = context
        .pointer_context
        .summary_hooks
        .and_then(|hooks| (hooks.reference_intrinsic)(symbol, context.svd, context.pointer_context))
    {
        if let Some(flow) = trace.reference_flow.take() {
            let original_flow = flow.clone();
            match compose_calls_in_reference_flow(
                flow,
                context,
                visiting,
                &mut trace.reference_dependencies,
            ) {
                Ok(flow) if reference_flow_calls_are_valid(&flow) => {
                    trace.reference_flow = Some(flow);
                }
                Ok(flow) => {
                    let detail = reference_flow_call_validation_error(&flow)
                        .unwrap_or_else(|| "unknown validation failure".to_owned());
                    trace.reference_flow = Some(flow);
                    trace.reference_blockers.push(format!(
                        "reviewed-summary: composed call result is used without a modeled callee `a0`: {detail}"
                    ));
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
        context.svd,
        context.relocated_calls,
        context.pointer_context,
        specialized_arguments,
        context.budget,
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
                    | DraftReferenceEvent::ModeledDirectCall { .. }
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
        match explore_reference_flow(symbol, context, specialized_arguments, visiting) {
            Ok(explored) => {
                trace
                    .reference_dependencies
                    .extend(explored.reference_dependencies.iter().cloned());
                trace.reference_dependencies.sort();
                trace.reference_dependencies.dedup();
                let uncomposed_flow = explored.flow.clone();
                let mut incomplete_effects = explored.incomplete_effects;
                let composed = compose_calls_in_reference_flow(
                    explored.flow,
                    context,
                    visiting,
                    &mut trace.reference_dependencies,
                );
                let flow = match composed {
                    Ok(flow) if reference_flow_calls_are_valid(&flow) => flow,
                    Ok(flow) => {
                        let detail = reference_flow_call_validation_error(&flow)
                            .unwrap_or_else(|| "unknown validation failure".to_owned());
                        incomplete_effects.push(format!(
                            "symbolic-cfg: composed call result is used without a modeled callee `a0`: {detail}"
                        ));
                        flow
                    }
                    Err(error) => {
                        incomplete_effects.push(format!(
                            "symbolic-cfg-call-composition: {error}; retained uncomposed structured flow as non-executable evidence"
                        ));
                        uncomposed_flow
                    }
                };
                trace.events.clear();
                trace.reference_events.clear();
                trace.located_events = explored.located_events;
                trace.located_reference_events = explored.located_reference_events;
                trace.blockers.clear();
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

    flatten_reference_trace(trace, context, specialized_arguments, visiting)
}
