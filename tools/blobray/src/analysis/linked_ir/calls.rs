//! Call normalization, semantic annotations, and flow call collection.

use super::*;

mod abi;

pub(super) use abi::{
    direct_semantic_typed_arguments, external_return_model, linked_event_dispatch_contract,
};
use abi::{
    external_return_is_modeled, linked_external_execution_model, reviewed_external_typed_arguments,
};

pub(super) fn canonical_arguments(arguments: &[SymbolicValue]) -> Vec<String> {
    arguments.iter().map(SymbolicValue::canonical).collect()
}

pub(super) fn argument_exactness(arguments: &[SymbolicValue]) -> Vec<bool> {
    arguments.iter().map(SymbolicValue::is_resolved).collect()
}

fn linked_flow_value(value: &SymbolicValue) -> LinkedFlowValue {
    LinkedFlowValue {
        expression: value.canonical(),
        constant: value.as_constant(),
        input: value.direct_input_index(),
    }
}

pub(super) fn local_value_flow(trace: &FunctionAnalysis) -> Vec<LinkedLocalValueFlow> {
    let mut facts = trace
        .located_reference_events
        .iter()
        .filter_map(|located| match &located.event {
            DraftReferenceEvent::PrivateStackStore {
                offset,
                width,
                value,
            } => Some(LinkedLocalValueFlow::StackStore {
                site: located.site,
                offset: *offset,
                width: *width,
                value: linked_flow_value(value),
            }),
            DraftReferenceEvent::PrivateStackLoad {
                token,
                offset,
                width,
                signed,
            } => Some(LinkedLocalValueFlow::StackLoad {
                site: located.site,
                token: *token,
                offset: *offset,
                width: *width,
                signed: *signed,
            }),
            _ => None,
        })
        .collect::<Vec<_>>();
    facts.sort();
    facts.dedup();
    facts
}

pub(super) fn affine_argument_bindings(arguments: &[SymbolicValue]) -> Vec<LinkedArgumentBinding> {
    arguments
        .iter()
        .enumerate()
        .filter_map(|(position, value)| {
            let (caller_argument, offset) = value.caller_memory_location()?;
            Some(LinkedArgumentBinding {
                position,
                caller_argument,
                offset,
                expression: match offset.cmp(&0) {
                    std::cmp::Ordering::Less => {
                        format!("arg{caller_argument} - {:#x}", offset.unsigned_abs())
                    }
                    std::cmp::Ordering::Equal => format!("arg{caller_argument}"),
                    std::cmp::Ordering::Greater => {
                        format!("arg{caller_argument} + {offset:#x}")
                    }
                },
            })
        })
        .collect()
}

pub(super) fn branch_operation(operation: BranchOperation) -> &'static str {
    match operation {
        BranchOperation::Equal => "equal",
        BranchOperation::NotEqual => "not-equal",
        BranchOperation::LessSigned => "less-signed",
        BranchOperation::GreaterEqualSigned => "greater-equal-signed",
        BranchOperation::LessUnsigned => "less-unsigned",
        BranchOperation::GreaterEqualUnsigned => "greater-equal-unsigned",
    }
}

pub(crate) fn effective_branch_operation(operation: &'static str, taken: bool) -> &'static str {
    if taken {
        return operation;
    }
    match operation {
        "equal" => "not-equal",
        "not-equal" => "equal",
        "less-signed" => "greater-equal-signed",
        "greater-equal-signed" => "less-signed",
        "less-unsigned" => "greater-equal-unsigned",
        "greater-equal-unsigned" => "less-unsigned",
        _ => unreachable!("branch operations have a closed vocabulary"),
    }
}

pub(super) fn format_guard_literal(guard: &LinkedCallGuard) -> String {
    if guard.taken {
        format!("({})", guard.condition)
    } else {
        format!("!({})", guard.condition)
    }
}

pub(crate) fn format_guard_path(path: &LinkedCallGuardPath) -> String {
    let expression = path
        .guards
        .iter()
        .map(format_guard_literal)
        .collect::<Vec<_>>()
        .join(" && ");
    if expression.is_empty() {
        "true".to_owned()
    } else {
        expression
    }
}

pub(crate) fn format_guard_paths(paths: &[LinkedCallGuardPath]) -> String {
    if paths.is_empty() {
        return "false".to_owned();
    }
    paths
        .iter()
        .map(|path| format!("({})", format_guard_path(path)))
        .collect::<Vec<_>>()
        .join(" || ")
}

pub(super) fn format_guard_path_without(path: &LinkedCallGuardPath, excluded: usize) -> String {
    let expression = path
        .guards
        .iter()
        .enumerate()
        .filter(|(index, _)| *index != excluded)
        .map(|(_, guard)| format_guard_literal(guard))
        .collect::<Vec<_>>()
        .join(" && ");
    if expression.is_empty() {
        "true".to_owned()
    } else {
        expression
    }
}

pub(super) fn branch_expression(condition: &BranchCondition) -> String {
    let left = pseudo_value(&condition.left);
    let right = pseudo_value(&condition.right);
    match condition.operation {
        BranchOperation::Equal => format!("{left} == {right}"),
        BranchOperation::NotEqual => format!("{left} != {right}"),
        BranchOperation::LessSigned => format!("({left} as i32) < ({right} as i32)"),
        BranchOperation::GreaterEqualSigned => format!("({left} as i32) >= ({right} as i32)"),
        BranchOperation::LessUnsigned => format!("{left} < {right}"),
        BranchOperation::GreaterEqualUnsigned => format!("{left} >= {right}"),
    }
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(super) struct LinkedCallIdentity {
    kind: &'static str,
    target: String,
    site: Option<u32>,
    direct: bool,
    tail: bool,
    result_modeled: bool,
    execution_model: Option<LinkedExternalExecutionModel>,
    semantics: Option<String>,
    semantic_operation: Option<String>,
    semantic_contract: Option<LinkedSemanticContract>,
    replacement_hint: Option<String>,
    project_symbol: Option<String>,
    project_candidates: Vec<String>,
    trampoline: Option<LinkedTrampoline>,
    typed_signature: Vec<(usize, String, String, &'static str)>,
}

impl From<&LinkedCall> for LinkedCallIdentity {
    fn from(call: &LinkedCall) -> Self {
        Self {
            kind: call.kind,
            target: call.target.clone(),
            site: call.site,
            direct: call.direct,
            tail: call.tail,
            result_modeled: call.result_modeled,
            execution_model: call.execution_model.clone(),
            semantics: call.semantics.clone(),
            semantic_operation: call.semantic_operation.clone(),
            semantic_contract: call.semantic_contract.clone(),
            replacement_hint: call.replacement_hint.clone(),
            project_symbol: call.project_symbol.clone(),
            project_candidates: call.project_candidates.clone(),
            trampoline: call.trampoline.clone(),
            typed_signature: call
                .typed_arguments
                .iter()
                .map(|argument| {
                    (
                        argument.position,
                        argument.name.clone(),
                        argument.c_type.clone(),
                        argument.direction,
                    )
                })
                .collect(),
        }
    }
}

pub(super) fn merged_argument_value(
    calls: &[LinkedCall],
    position: usize,
    argument_shapes: usize,
) -> String {
    if let Some(first) = calls[0].arguments.get(position)
        && calls
            .iter()
            .all(|call| call.arguments.get(position) == Some(first))
    {
        return first.clone();
    }
    let constants = calls
        .iter()
        .filter_map(|call| call.arguments.get(position))
        .filter_map(|value| value.strip_prefix("const:"))
        .collect::<BTreeSet<_>>();
    if constants.len() == calls.len() {
        return format!(
            "one-of({})",
            constants.into_iter().collect::<Vec<_>>().join(",")
        );
    }
    format!("varies-across-{argument_shapes}-shapes")
}

pub(super) fn merged_typed_argument_value(
    calls: &[LinkedCall],
    position: usize,
    argument_shapes: usize,
) -> String {
    fn value(call: &LinkedCall, position: usize) -> Option<&str> {
        call.typed_arguments
            .iter()
            .find(|argument| argument.position == position)
            .map(|argument| argument.value.as_str())
    }

    if let Some(first) = value(&calls[0], position)
        && calls
            .iter()
            .all(|call| value(call, position) == Some(first))
    {
        return first.to_owned();
    }
    format!("varies-across-{argument_shapes}-shapes")
}

pub(super) fn normalize_guard_paths(
    paths: impl IntoIterator<Item = LinkedCallGuardPath>,
) -> Vec<LinkedCallGuardPath> {
    let mut paths = paths
        .into_iter()
        .map(|mut path| {
            path.guards.sort();
            path.guards.dedup();
            path
        })
        .collect::<BTreeSet<_>>();

    loop {
        let snapshot = paths.iter().cloned().collect::<Vec<_>>();
        let mut consensus = None;
        'pairs: for (index, left) in snapshot.iter().enumerate() {
            for right in &snapshot[index + 1..] {
                if left.guards.len() != right.guards.len() {
                    continue;
                }
                let mut differing = None;
                let mut compatible = true;
                for (guard_index, (left, right)) in
                    left.guards.iter().zip(&right.guards).enumerate()
                {
                    if left.site != right.site
                        || left.condition != right.condition
                        || left.result_sources != right.result_sources
                    {
                        compatible = false;
                        break;
                    }
                    if left.taken != right.taken && differing.replace(guard_index).is_some() {
                        compatible = false;
                        break;
                    }
                }
                if compatible && let Some(differing) = differing {
                    let mut guards = left.guards.clone();
                    guards.remove(differing);
                    consensus = Some(LinkedCallGuardPath { guards });
                    break 'pairs;
                }
            }
        }
        let Some(consensus) = consensus else {
            break;
        };
        let previous_len = paths.len();
        paths.insert(consensus);
        let snapshot = paths.iter().cloned().collect::<Vec<_>>();
        paths.retain(|path| {
            !snapshot.iter().any(|candidate| {
                candidate != path
                    && candidate.guards.len() <= path.guards.len()
                    && candidate
                        .guards
                        .iter()
                        .all(|guard| path.guards.contains(guard))
            })
        });
        if paths.len() == previous_len {
            break;
        }
    }

    let snapshot = paths.iter().cloned().collect::<Vec<_>>();
    paths.retain(|path| {
        !snapshot.iter().any(|candidate| {
            candidate != path
                && candidate.guards.len() <= path.guards.len()
                && candidate
                    .guards
                    .iter()
                    .all(|guard| path.guards.contains(guard))
        })
    });
    paths.into_iter().collect()
}

pub(super) fn merged_guard_paths(calls: &[LinkedCall]) -> Option<Vec<LinkedCallGuardPath>> {
    let mut paths = Vec::new();
    for call in calls {
        paths.extend(call.guard_paths.as_ref()?.iter().cloned());
    }
    Some(normalize_guard_paths(paths))
}

pub(super) fn distinct_argument_shape_count(calls: &[LinkedCall]) -> usize {
    let mut shapes = BTreeMap::<
        (
            Vec<String>,
            Vec<bool>,
            Vec<LinkedArgumentBinding>,
            Vec<(usize, String)>,
        ),
        usize,
    >::new();
    for call in calls {
        let shape = (
            call.arguments.clone(),
            call.argument_exact.clone(),
            call.argument_bindings.clone(),
            call.typed_arguments
                .iter()
                .map(|argument| (argument.position, argument.value.clone()))
                .collect(),
        );
        shapes
            .entry(shape)
            .and_modify(|count| *count = (*count).max(call.argument_shapes))
            .or_insert(call.argument_shapes);
    }
    shapes.into_values().sum()
}

pub(super) fn compact_calls(calls: impl IntoIterator<Item = LinkedCall>) -> Vec<LinkedCall> {
    let mut groups = BTreeMap::<LinkedCallIdentity, Vec<LinkedCall>>::new();
    for call in calls {
        groups
            .entry(LinkedCallIdentity::from(&call))
            .or_default()
            .push(call);
    }

    groups
        .into_values()
        .map(|calls| {
            let argument_shapes = distinct_argument_shape_count(&calls);
            let argument_count = calls
                .iter()
                .map(|call| call.arguments.len())
                .max()
                .unwrap_or_default();
            let arguments = (0..argument_count)
                .map(|position| merged_argument_value(&calls, position, argument_shapes))
                .collect::<Vec<_>>();
            let argument_exact = (0..argument_count)
                .map(|position| {
                    calls.iter().all(|call| {
                        call.argument_exact.get(position).copied() == Some(true)
                            && call.arguments.get(position) == arguments.get(position)
                    })
                })
                .collect();
            let argument_bindings = calls[0]
                .argument_bindings
                .iter()
                .filter(|binding| {
                    calls[1..]
                        .iter()
                        .all(|call| call.argument_bindings.contains(binding))
                })
                .cloned()
                .collect();
            let mut call = calls[0].clone();
            for argument in &mut call.typed_arguments {
                argument.value =
                    merged_typed_argument_value(&calls, argument.position, argument_shapes);
            }
            call.argument_shapes = argument_shapes;
            call.arguments = arguments;
            call.argument_exact = argument_exact;
            call.argument_bindings = argument_bindings;
            call.guard_paths = merged_guard_paths(&calls);
            call
        })
        .collect()
}

pub(super) fn collect_call_event(
    event: &DraftReferenceEvent,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    calls: &mut BTreeSet<LinkedCall>,
) {
    let call = match event {
        DraftReferenceEvent::ReviewedExternalCall {
            site,
            candidates,
            arguments,
            ..
        } => {
            let evidence = candidates
                .iter()
                .map(|candidate| candidate.evidence)
                .collect::<BTreeSet<_>>();
            let evidence = (evidence.len() == 1)
                .then(|| *evidence.first().expect("one reviewed evidence kind"));
            let tails = candidates
                .iter()
                .map(|candidate| candidate.tail)
                .collect::<BTreeSet<_>>();
            let operations = candidates
                .iter()
                .filter_map(|candidate| candidate.semantic_operation.as_deref())
                .collect::<BTreeSet<_>>();
            let semantic_operation = (operations.len() == 1)
                .then(|| (*operations.first().expect("one reviewed operation")).to_owned());
            let replacements = candidates
                .iter()
                .filter_map(|candidate| candidate.replacement_hint.as_deref())
                .collect::<BTreeSet<_>>();
            let execution_models = candidates
                .iter()
                .filter_map(|candidate| candidate.execution_model.as_ref())
                .collect::<BTreeSet<_>>();
            let execution_model = (candidates.len() == 1 && execution_models.len() == 1)
                .then(|| *execution_models.first().expect("one execution model"));
            Some(LinkedCall {
                kind: "reviewed-external",
                target: candidates
                    .iter()
                    .map(|candidate| format!("{}::{}", candidate.contract, candidate.name))
                    .collect::<Vec<_>>()
                    .join(" | "),
                site: Some(*site),
                direct: false,
                tail: tails.len() == 1 && *tails.first().expect("one reviewed call shape"),
                result_modeled: execution_model
                    .is_some_and(|model| external_return_is_modeled(model.return_model)),
                execution_model: execution_model.map(linked_external_execution_model),
                semantics: Some(format!(
                    "reviewed ABI; candidates={}; executable-model={}",
                    candidates
                        .iter()
                        .map(|candidate| format!(
                            "{}({}) -> {}{}",
                            candidate.name,
                            candidate.argument_types.join(", "),
                            candidate.return_type,
                            if candidate.variadic { " variadic" } else { "" }
                        ))
                        .collect::<Vec<_>>()
                        .join(" | "),
                    execution_model.map_or("none", |model| model.id.as_str()),
                )),
                semantic_operation,
                semantic_contract: Some(LinkedSemanticContract {
                    source: evidence.map_or("ambiguous-reviewed-interface-evidence", |evidence| {
                        evidence.source()
                    }),
                    id: candidates
                        .iter()
                        .map(|candidate| candidate.id.as_str())
                        .collect::<Vec<_>>()
                        .join(" | "),
                    evidence: evidence.map_or_else(
                        || "ambiguous-reviewed-interface-evidence".to_owned(),
                        |evidence| evidence.description().to_owned(),
                    ),
                    body_policy: "opaque-boundary",
                    event_dispatch: None,
                }),
                replacement_hint: (replacements.len() == 1)
                    .then(|| (*replacements.first().expect("one replacement hint")).to_owned()),
                project_symbol: None,
                project_candidates: Vec::new(),
                trampoline: None,
                argument_shapes: 1,
                arguments: canonical_arguments(arguments),
                argument_exact: argument_exactness(arguments),
                argument_bindings: affine_argument_bindings(arguments),
                typed_arguments: reviewed_external_typed_arguments(candidates, arguments),
                guard_paths: None,
            })
        }
        DraftReferenceEvent::ModeledDirectCall {
            site,
            function,
            arguments,
            ..
        } => Some(LinkedCall {
            kind: "modeled-direct-external",
            target: function.name.clone(),
            site: Some(*site),
            direct: true,
            tail: false,
            result_modeled: external_return_is_modeled(function.return_model),
            execution_model: None,
            semantics: Some(format!(
                "reviewed direct platform ABI; args={}; return={}; model={}; operation={}",
                function.argument_count,
                function.return_type,
                external_return_model(function.return_model),
                function.operation,
            )),
            semantic_operation: Some(function.operation.clone()),
            semantic_contract: Some(LinkedSemanticContract {
                source: "reviewed-direct-external-model",
                id: function.id.clone(),
                evidence: function.evidence.clone(),
                body_policy: "opaque-boundary",
                event_dispatch: None,
            }),
            replacement_hint: function.replacement_hint.clone(),
            project_symbol: Some(function.name.clone()),
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_exact: argument_exactness(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        DraftReferenceEvent::DiagnosticCall {
            site,
            function,
            arguments,
            ..
        } => Some(LinkedCall {
            kind: "diagnostic",
            target: function.clone(),
            site: Some(*site),
            direct: false,
            tail: false,
            result_modeled: false,
            execution_model: None,
            semantics: Some("diagnostic/logging boundary".to_owned()),
            semantic_operation: Some("diagnostic.emit".to_owned()),
            semantic_contract: Some(LinkedSemanticContract {
                source: "registered-diagnostic-symbol",
                id: function.clone(),
                evidence: "relocated-symbol-and-reviewed-arity".to_owned(),
                body_policy: "opaque-boundary",
                event_dispatch: None,
            }),
            replacement_hint: Some("Rust logging/assertion boundary".to_owned()),
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_exact: argument_exactness(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        DraftReferenceEvent::Call {
            site,
            target,
            direct,
            arguments,
            ..
        } => Some(LinkedCall {
            kind: if resolver.symbols_by_address.contains_key(target) {
                "internal"
            } else {
                "unresolved"
            },
            target: identities.target(*target),
            site: Some(*site),
            direct: *direct,
            tail: false,
            result_modeled: false,
            execution_model: None,
            semantics: None,
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_exact: argument_exactness(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        DraftReferenceEvent::TailCall {
            site,
            target,
            direct,
            arguments,
            ..
        } => Some(LinkedCall {
            kind: if resolver.symbols_by_address.contains_key(target) {
                "internal"
            } else {
                "unresolved"
            },
            target: identities.target(*target),
            site: Some(*site),
            direct: *direct,
            tail: true,
            result_modeled: false,
            execution_model: None,
            semantics: None,
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_exact: argument_exactness(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        DraftReferenceEvent::ComposedCall {
            site,
            symbol,
            direct,
            tail,
            arguments,
            result_modeled,
            ..
        } => Some(LinkedCall {
            kind: "internal",
            target: symbol.clone(),
            site: Some(*site),
            direct: *direct,
            tail: *tail,
            result_modeled: *result_modeled,
            execution_model: None,
            semantics: Some("callee body was composed by the reference resolver".to_owned()),
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_exact: argument_exactness(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        DraftReferenceEvent::ScratchCall {
            site,
            target,
            direct,
            arguments,
            scratch_argument,
            scratch_size,
            ..
        } => Some(LinkedCall {
            kind: if resolver.symbols_by_address.contains_key(target) {
                "internal"
            } else {
                "unresolved"
            },
            target: identities.target(*target),
            site: Some(*site),
            direct: *direct,
            tail: false,
            result_modeled: false,
            execution_model: None,
            semantics: Some(format!(
                "scratch argument={scratch_argument} size={scratch_size}"
            )),
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_exact: argument_exactness(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        DraftReferenceEvent::ComposedCallWithScratch {
            site,
            symbol,
            direct,
            arguments,
            result_modeled,
            scratch_argument,
            scratch_size,
            ..
        } => Some(LinkedCall {
            kind: "internal",
            target: symbol.clone(),
            site: Some(*site),
            direct: *direct,
            tail: false,
            result_modeled: *result_modeled,
            execution_model: None,
            semantics: Some(format!(
                "composed callee with scratch argument={scratch_argument} size={scratch_size}"
            )),
            semantic_operation: None,
            semantic_contract: None,
            replacement_hint: None,
            project_symbol: None,
            project_candidates: Vec::new(),
            trampoline: None,
            argument_shapes: 1,
            arguments: canonical_arguments(arguments),
            argument_exact: argument_exactness(arguments),
            argument_bindings: affine_argument_bindings(arguments),
            typed_arguments: Vec::new(),
            guard_paths: None,
        }),
        _ => None,
    };
    if let Some(call) = call {
        calls.insert(call);
    }
}
