//! Best-effort linked function/call IR for manual vendor-code analysis.

use std::{
    collections::{BTreeMap, BTreeSet, VecDeque},
    fmt::Write as _,
};

use crate::{
    BranchCondition, BranchOperation, DraftReferenceEvent, DraftReferenceFlow,
    DraftReferenceTerminator, ExpressionOperation, ExternalReturnModel, FunctionAnalysis,
    MemoryAccess, MmioRegisterMap, ObservableEvent, ReferenceResolver, SymbolicValue, artifact,
    direct,
};

const MAX_CALL_GRAPH_STATES: usize = 127;
const MAX_CALL_GRAPH_BRANCH_DECISIONS: usize = 12;

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedCallArgument {
    pub(crate) position: usize,
    pub(crate) name: String,
    pub(crate) c_type: String,
    pub(crate) direction: &'static str,
    pub(crate) value: String,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedCall {
    pub(crate) kind: &'static str,
    pub(crate) target: String,
    pub(crate) site: Option<u32>,
    pub(crate) tail: bool,
    pub(crate) result_modeled: bool,
    pub(crate) semantics: Option<String>,
    pub(crate) semantic_operation: Option<String>,
    pub(crate) replacement_hint: Option<String>,
    pub(crate) arguments: Vec<String>,
    pub(crate) typed_arguments: Vec<LinkedCallArgument>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct ContextAccess {
    pub(crate) argument: u8,
    pub(crate) offset: i32,
    pub(crate) access: &'static str,
    pub(crate) width: u8,
    pub(crate) path: String,
    pub(crate) value: Option<String>,
    pub(crate) value_pseudo: Option<String>,
    pub(crate) write_mask: Option<u32>,
    pub(crate) preserved_mask: Option<u32>,
    pub(crate) forced_zero_mask: Option<u32>,
    pub(crate) forced_one_mask: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContextField {
    pub(crate) argument: u8,
    pub(crate) offset: i32,
    pub(crate) width: u8,
    pub(crate) reads: usize,
    pub(crate) writes: usize,
    pub(crate) write_mask: u32,
    pub(crate) paths: Vec<String>,
    pub(crate) write_values: Vec<String>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub(crate) struct LinkedMmioAccess {
    pub(crate) ordinal: usize,
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) register: String,
    pub(crate) access: &'static str,
    pub(crate) mode: &'static str,
    pub(crate) path: String,
    pub(crate) address_expression: Option<String>,
    pub(crate) guard: Option<String>,
    pub(crate) value: Option<String>,
    pub(crate) modified_mask: Option<u32>,
    pub(crate) preserved_mask: Option<u32>,
    pub(crate) inverted_mask: Option<u32>,
    pub(crate) forced_zero_mask: Option<u32>,
    pub(crate) forced_one_mask: Option<u32>,
    pub(crate) read_derived_mask: Option<u32>,
    pub(crate) dynamic_mask: Option<u32>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedMmioRegister {
    pub(crate) address: u32,
    pub(crate) width: u8,
    pub(crate) names: Vec<String>,
    pub(crate) read_shapes: usize,
    pub(crate) write_shapes: usize,
    pub(crate) poll_shapes: usize,
    pub(crate) static_shapes: usize,
    pub(crate) indexed_candidate_shapes: usize,
    pub(crate) functions: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SemanticBoundary {
    pub(crate) operation: String,
    pub(crate) call_shapes: usize,
    pub(crate) functions: Vec<String>,
    pub(crate) targets: Vec<String>,
    pub(crate) replacement_hints: Vec<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedIrFunction {
    pub(crate) source: String,
    pub(crate) identity: String,
    pub(crate) member: Option<String>,
    pub(crate) symbol: String,
    pub(crate) binding: &'static str,
    pub(crate) address: Option<u32>,
    pub(crate) object_offset: u32,
    pub(crate) size: usize,
    pub(crate) flow_kind: &'static str,
    pub(crate) complete: bool,
    pub(crate) exact: bool,
    pub(crate) return_value: String,
    pub(crate) dependencies: Vec<String>,
    pub(crate) calls: Vec<LinkedCall>,
    pub(crate) mmio_accesses: Vec<LinkedMmioAccess>,
    pub(crate) context_accesses: Vec<ContextAccess>,
    pub(crate) context_fields: Vec<ContextField>,
    pub(crate) call_graph_blockers: Vec<String>,
    pub(crate) direct_blockers: Vec<String>,
    pub(crate) reference_blockers: Vec<String>,
    pub(crate) pseudo: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct LinkedIrReport {
    pub(crate) functions: Vec<LinkedIrFunction>,
    pub(crate) mmio_registers: Vec<LinkedMmioRegister>,
    pub(crate) mmio_functions: usize,
    pub(crate) mmio_access_shapes: usize,
    pub(crate) semantic_boundaries: Vec<SemanticBoundary>,
    pub(crate) semantic_calls: usize,
    pub(crate) exported_functions: usize,
    pub(crate) local_functions: usize,
    pub(crate) context_functions: usize,
    pub(crate) context_accesses: usize,
    pub(crate) context_fields: usize,
    pub(crate) complete_functions: usize,
    pub(crate) structured_functions: usize,
    pub(crate) internal_calls: usize,
    pub(crate) external_calls: usize,
    pub(crate) unresolved_calls: usize,
}

fn identity(member: Option<&str>, symbol: &str) -> String {
    member.map_or_else(|| symbol.to_owned(), |member| format!("{member}:{symbol}"))
}

type SymbolKey = (Option<String>, String, u64);

fn symbol_key(symbol: &artifact::ArtifactSymbolDefinition) -> SymbolKey {
    (symbol.member.clone(), symbol.name.clone(), symbol.address)
}

struct IrIdentityCatalog {
    symbols: BTreeMap<SymbolKey, String>,
    targets: BTreeMap<u32, String>,
}

impl IrIdentityCatalog {
    fn new(resolver: &ReferenceResolver, namespace: Option<&str>) -> Self {
        let mut definitions = resolver.symbols.clone();
        definitions.extend(resolver.symbols_by_address.values().cloned());
        definitions.sort_by_key(symbol_key);
        definitions.dedup_by_key(|symbol| symbol_key(symbol));

        let mut base_counts = BTreeMap::<(Option<String>, String), usize>::new();
        for symbol in &definitions {
            *base_counts
                .entry((symbol.member.clone(), symbol.name.clone()))
                .or_default() += 1;
        }
        let symbols = definitions
            .iter()
            .map(|symbol| {
                let base = identity(symbol.member.as_deref(), &symbol.name);
                let duplicate = base_counts
                    .get(&(symbol.member.clone(), symbol.name.clone()))
                    .copied()
                    .unwrap_or_default()
                    > 1;
                let value = if duplicate {
                    format!("{base}@{:#010x}", symbol.address as u32)
                } else {
                    base
                };
                let value = namespace.map_or(value.clone(), |source| format!("{source}::{value}"));
                (symbol_key(symbol), value)
            })
            .collect::<BTreeMap<_, _>>();
        let targets = resolver
            .symbols_by_address
            .iter()
            .map(|(target, symbol)| {
                (
                    *target,
                    symbols
                        .get(&symbol_key(symbol))
                        .expect("target symbol is present in IR identity catalog")
                        .clone(),
                )
            })
            .collect();
        Self { symbols, targets }
    }

    fn symbol(&self, symbol: &artifact::ArtifactSymbolDefinition) -> String {
        self.symbols
            .get(&symbol_key(symbol))
            .expect("IR symbol is present in identity catalog")
            .clone()
    }

    fn target(&self, target: u32) -> String {
        self.targets
            .get(&target)
            .cloned()
            .unwrap_or_else(|| format!("sub_{target:08x}"))
    }
}

fn pseudo_identifier(value: &str) -> String {
    let mut output = String::new();
    for character in value.chars() {
        if character.is_ascii_alphanumeric() || character == '_' {
            output.push(character);
        } else {
            output.push('_');
        }
    }
    if output.is_empty() {
        "unnamed".to_owned()
    } else if output.as_bytes()[0].is_ascii_digit() {
        format!("fn_{output}")
    } else {
        output
    }
}

fn pseudo_value(value: &SymbolicValue) -> String {
    if let Some(index) = value.direct_input_index() {
        return format!("arg{index}");
    }
    match value {
        SymbolicValue::Unknown => "unknown".to_owned(),
        SymbolicValue::Constant(value) | SymbolicValue::InputConstant { value, .. } => {
            format!("{value:#010x}")
        }
        SymbolicValue::StackAddress(offset) => format!("stack.ptr({offset:+#x})"),
        SymbolicValue::SymbolAddress {
            member,
            symbol,
            hi_addend,
            lo_addend,
            post_offset,
        } => format!(
            "symbol({}::{symbol}, hi={hi_addend:+#x}, lo={}, post={post_offset:+#x})",
            member.as_deref().unwrap_or("linked"),
            lo_addend.map_or_else(|| "?".to_owned(), |value| format!("{value:+#x}"))
        ),
        SymbolicValue::CallResult(token) => format!("call{token}"),
        SymbolicValue::ExternalResult(token) => format!("external{token}"),
        SymbolicValue::Expression {
            operation,
            left,
            right,
        } => {
            let left = pseudo_value(left);
            let right = pseudo_value(right);
            match operation {
                ExpressionOperation::Add => format!("{left}.wrapping_add({right})"),
                ExpressionOperation::Subtract => format!("{left}.wrapping_sub({right})"),
                ExpressionOperation::Multiply => format!("{left}.wrapping_mul({right})"),
                ExpressionOperation::DivideSigned => format!("signed_div({left}, {right})"),
                ExpressionOperation::DivideUnsigned => format!("{left} / {right}"),
                ExpressionOperation::RemainderSigned => format!("signed_rem({left}, {right})"),
                ExpressionOperation::RemainderUnsigned => format!("{left} % {right}"),
                ExpressionOperation::BitAnd => format!("({left} & {right})"),
                ExpressionOperation::BitOr => format!("({left} | {right})"),
                ExpressionOperation::BitXor => format!("({left} ^ {right})"),
                ExpressionOperation::ShiftLeft => format!("({left} << ({right} & 31))"),
                ExpressionOperation::ShiftRight => format!("({left} >> ({right} & 31))"),
                ExpressionOperation::ShiftRightArithmetic => {
                    format!("(({left} as i32) >> ({right} & 31)) as u32")
                }
                ExpressionOperation::Equal => format!("u32::from({left} == {right})"),
                ExpressionOperation::LessThanSigned => {
                    format!("u32::from(({left} as i32) < ({right} as i32))")
                }
                ExpressionOperation::LessThanUnsigned => {
                    format!("u32::from({left} < {right})")
                }
            }
        }
        SymbolicValue::WideSignedDivide {
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
            high_word,
        } => format!(
            "sdiv64_{}({}, {}, {}, {})",
            if *high_word { "high" } else { "low" },
            pseudo_value(dividend_low),
            pseudo_value(dividend_high),
            pseudo_value(divisor_low),
            pseudo_value(divisor_high)
        ),
        SymbolicValue::RegisterImage {
            read_token,
            and_mask,
            or_mask,
            ..
        }
        | SymbolicValue::IndexedRegisterImage {
            read_token,
            and_mask,
            or_mask,
        } => format!("((read{read_token} & {and_mask:#010x}) | {or_mask:#010x})"),
        SymbolicValue::MemoryImage {
            read_token,
            and_mask,
            or_mask,
        } => format!("((ramread{read_token} & {and_mask:#010x}) | {or_mask:#010x})"),
        SymbolicValue::ExternalTable(_)
        | SymbolicValue::ExternalFunction { .. }
        | SymbolicValue::FunctionTable(_)
        | SymbolicValue::FunctionPointer { .. }
        | SymbolicValue::Bits(_) => format!("symbolic({:?})", value.canonical()),
    }
}

fn pseudo_arguments(arguments: &[SymbolicValue]) -> String {
    arguments
        .iter()
        .map(pseudo_value)
        .collect::<Vec<_>>()
        .join(", ")
}

fn pseudo_external_arguments(
    function: crate::ExternalFunctionRef,
    arguments: &[SymbolicValue],
) -> String {
    let semantic = function.spec().semantic;
    arguments
        .iter()
        .enumerate()
        .map(|(position, value)| {
            semantic.arguments.get(position).map_or_else(
                || pseudo_value(value),
                |argument| {
                    format!(
                        "{} /* {} {:?} */ = {}",
                        argument.name,
                        argument.c_type,
                        argument.direction,
                        pseudo_value(value)
                    )
                },
            )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn canonical_arguments(arguments: &[SymbolicValue]) -> Vec<String> {
    arguments.iter().map(SymbolicValue::canonical).collect()
}

fn external_typed_arguments(
    function: crate::ExternalFunctionRef,
    arguments: &[SymbolicValue],
) -> Vec<LinkedCallArgument> {
    let function = function.spec();
    arguments
        .iter()
        .enumerate()
        .map(|(position, value)| {
            let semantic = function.semantic.arguments.get(position);
            LinkedCallArgument {
                position,
                name: semantic.map_or_else(
                    || format!("arg{position}"),
                    |argument| argument.name.to_owned(),
                ),
                c_type: semantic
                    .map_or_else(|| "u32".to_owned(), |argument| argument.c_type.to_owned()),
                direction: semantic.map_or("unknown", |argument| match argument.direction {
                    crate::ExternalArgumentDirection::Input => "input",
                    crate::ExternalArgumentDirection::Output => "output",
                    crate::ExternalArgumentDirection::InputOutput => "input-output",
                }),
                value: value.canonical(),
            }
        })
        .collect()
}

fn branch_expression(condition: &BranchCondition) -> String {
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

fn external_semantics(event: &DraftReferenceEvent) -> Option<String> {
    let DraftReferenceEvent::ExternalCall {
        table, function, ..
    } = event
    else {
        return None;
    };
    let table = table.spec();
    let function = function.spec();
    Some(format!(
        "table={} version={} slot={:#x} args={} return={:?} operation={}",
        table.id,
        table.version,
        function.offset,
        function.argument_count,
        function.return_model,
        function.semantic.operation,
    ))
}

fn collect_call_event(
    event: &DraftReferenceEvent,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    calls: &mut BTreeSet<LinkedCall>,
) {
    let call = match event {
        DraftReferenceEvent::ExternalCall {
            table,
            function,
            arguments,
            ..
        } => Some(LinkedCall {
            kind: "external",
            target: format!("{}::{}", table.spec().id, function.spec().c_name),
            site: None,
            tail: false,
            result_modeled: matches!(
                function.spec().return_model,
                ExternalReturnModel::Constant(_) | ExternalReturnModel::SymbolicU32
            ),
            semantics: external_semantics(event),
            semantic_operation: Some(function.spec().semantic.operation.to_owned()),
            replacement_hint: function.spec().semantic.replacement.map(str::to_owned),
            arguments: canonical_arguments(arguments),
            typed_arguments: external_typed_arguments(*function, arguments),
        }),
        DraftReferenceEvent::DiagnosticCall {
            function,
            arguments,
            ..
        } => Some(LinkedCall {
            kind: "diagnostic",
            target: function.clone(),
            site: None,
            tail: false,
            result_modeled: false,
            semantics: Some("diagnostic/logging boundary".to_owned()),
            semantic_operation: Some("diagnostic.emit".to_owned()),
            replacement_hint: Some("Rust logging/assertion boundary".to_owned()),
            arguments: canonical_arguments(arguments),
            typed_arguments: Vec::new(),
        }),
        DraftReferenceEvent::Call {
            site,
            target,
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
            tail: false,
            result_modeled: false,
            semantics: None,
            semantic_operation: None,
            replacement_hint: None,
            arguments: canonical_arguments(arguments),
            typed_arguments: Vec::new(),
        }),
        DraftReferenceEvent::TailCall {
            site,
            target,
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
            tail: true,
            result_modeled: false,
            semantics: None,
            semantic_operation: None,
            replacement_hint: None,
            arguments: canonical_arguments(arguments),
            typed_arguments: Vec::new(),
        }),
        DraftReferenceEvent::ComposedCall {
            symbol,
            arguments,
            result_modeled,
            ..
        } => Some(LinkedCall {
            kind: "internal",
            target: symbol.clone(),
            site: None,
            tail: false,
            result_modeled: *result_modeled,
            semantics: Some("callee body was composed by the reference resolver".to_owned()),
            semantic_operation: None,
            replacement_hint: None,
            arguments: canonical_arguments(arguments),
            typed_arguments: Vec::new(),
        }),
        DraftReferenceEvent::ScratchCall {
            site,
            target,
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
            tail: false,
            result_modeled: false,
            semantics: Some(format!(
                "scratch argument={scratch_argument} size={scratch_size}"
            )),
            semantic_operation: None,
            replacement_hint: None,
            arguments: canonical_arguments(arguments),
            typed_arguments: Vec::new(),
        }),
        DraftReferenceEvent::ComposedCallWithScratch {
            symbol,
            arguments,
            result_modeled,
            scratch_argument,
            scratch_size,
            ..
        } => Some(LinkedCall {
            kind: "internal",
            target: symbol.clone(),
            site: None,
            tail: false,
            result_modeled: *result_modeled,
            semantics: Some(format!(
                "composed callee with scratch argument={scratch_argument} size={scratch_size}"
            )),
            semantic_operation: None,
            replacement_hint: None,
            arguments: canonical_arguments(arguments),
            typed_arguments: Vec::new(),
        }),
        _ => None,
    };
    if let Some(call) = call {
        calls.insert(call);
    }
}

#[derive(Default)]
struct DirectCallGraph {
    calls: BTreeSet<LinkedCall>,
    blockers: BTreeSet<String>,
}

fn explore_direct_calls(
    symbol: &artifact::ArtifactSymbolDefinition,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    svd: &MmioRegisterMap,
) -> DirectCallGraph {
    let mut result = DirectCallGraph::default();
    let mut queue = VecDeque::from([BTreeMap::<u32, bool>::new()]);
    let mut queued = BTreeSet::from([BTreeMap::<u32, bool>::new()]);
    let mut explored_states = 0usize;

    while let Some(forced_branches) = queue.pop_front() {
        if explored_states >= MAX_CALL_GRAPH_STATES {
            result.blockers.insert(format!(
                "call graph exceeds the exploration limit of {MAX_CALL_GRAPH_STATES} states"
            ));
            break;
        }
        explored_states += 1;
        let trace = match direct::trace_binary_symbol_with_branches(
            symbol,
            svd,
            &resolver.relocated_calls,
            &resolver.pointer_context,
            None,
            &forced_branches,
        ) {
            Ok(trace) => trace,
            Err(error) => {
                result.blockers.insert(error.to_string());
                continue;
            }
        };
        for event in &trace.reference_events {
            collect_call_event(event, resolver, identities, &mut result.calls);
        }
        result
            .blockers
            .extend(trace.reference_blockers.iter().cloned());

        let Some(branch) = trace.unresolved_branch else {
            continue;
        };
        if forced_branches.len() >= MAX_CALL_GRAPH_BRANCH_DECISIONS {
            result.blockers.insert(format!(
                "call graph exceeds the limit of {MAX_CALL_GRAPH_BRANCH_DECISIONS} branch decisions per path at {:#010x}",
                branch.site
            ));
            continue;
        }
        for taken in [false, true] {
            let mut next = forced_branches.clone();
            if next.insert(branch.site, taken).is_some() {
                result.blockers.insert(format!(
                    "call graph revisits branch {:#010x}; that path is incomplete",
                    branch.site
                ));
            } else if queued.insert(next.clone()) {
                queue.push_back(next);
            }
        }
    }

    result
}

fn collect_calls_from_event(
    event: &DraftReferenceEvent,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    calls: &mut BTreeSet<LinkedCall>,
) {
    collect_call_event(event, resolver, identities, calls);
    match event {
        DraftReferenceEvent::BoundedPoll {
            body, on_exhausted, ..
        } => {
            collect_calls_from_flow(body, resolver, identities, calls);
            if let Some(event) = on_exhausted.as_deref() {
                collect_calls_from_event(event, resolver, identities, calls);
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_calls_from_flow(body, resolver, identities, calls);
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            for flow in [initial_read, setup, write_candidate, sample] {
                collect_calls_from_flow(flow, resolver, identities, calls);
            }
        }
        // A composed call's nested flow belongs to the callee. The caller edge
        // above is direct; recursively collecting it would create transitive
        // edges and obscure the actual call graph.
        _ => {}
    }
}

fn collect_calls_from_flow(
    flow: &DraftReferenceFlow,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
    calls: &mut BTreeSet<LinkedCall>,
) {
    for event in &flow.events {
        collect_calls_from_event(event, resolver, identities, calls);
    }
    if let DraftReferenceTerminator::Branch {
        taken, not_taken, ..
    } = &flow.terminator
    {
        collect_calls_from_flow(taken, resolver, identities, calls);
        collect_calls_from_flow(not_taken, resolver, identities, calls);
    }
}

fn nested_path(path: &str, scope: &str) -> String {
    format!("{path} / {scope}")
}

fn width_mask(width: u8) -> u32 {
    match width {
        8 => 0xff,
        16 => 0xffff,
        32 => u32::MAX,
        _ => 0,
    }
}

fn context_write_masks(
    access: MemoryAccess,
    width: u8,
    value: Option<&SymbolicValue>,
) -> (Option<u32>, Option<u32>, Option<u32>, Option<u32>) {
    if access != MemoryAccess::Write {
        return (None, None, None, None);
    }
    let width_mask = width_mask(width);
    let Some(SymbolicValue::MemoryImage {
        and_mask, or_mask, ..
    }) = value
    else {
        return (Some(width_mask), None, None, None);
    };
    let forced_one = or_mask & width_mask;
    let preserved = and_mask & !forced_one & width_mask;
    let forced_zero = width_mask & !(preserved | forced_one);
    (
        Some(forced_zero | forced_one),
        Some(preserved),
        Some(forced_zero),
        Some(forced_one),
    )
}

fn collect_context_access_from_event(
    event: &DraftReferenceEvent,
    path: &str,
    output: &mut Vec<ContextAccess>,
) {
    match event {
        DraftReferenceEvent::Memory {
            access,
            width,
            address,
            value,
            ..
        } => {
            if let Some((argument, offset)) = address.caller_memory_location() {
                let (write_mask, preserved_mask, forced_zero_mask, forced_one_mask) =
                    context_write_masks(*access, *width, value.as_ref());
                output.push(ContextAccess {
                    argument,
                    offset,
                    access: match access {
                        MemoryAccess::Read => "read",
                        MemoryAccess::Write => "write",
                    },
                    width: *width,
                    path: path.to_owned(),
                    value: value.as_ref().map(SymbolicValue::canonical),
                    value_pseudo: value.as_ref().map(pseudo_value),
                    write_mask,
                    preserved_mask,
                    forced_zero_mask,
                    forced_one_mask,
                });
            }
        }
        DraftReferenceEvent::BoundedPoll {
            body, on_exhausted, ..
        } => {
            collect_context_access_from_flow(body, &nested_path(path, "bounded-poll"), output);
            if let Some(event) = on_exhausted.as_deref() {
                collect_context_access_from_event(
                    event,
                    &nested_path(path, "poll-exhausted"),
                    output,
                );
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_context_access_from_flow(body, &nested_path(path, "poll"), output);
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            for (scope, flow) in [
                ("calibration-initial-read", initial_read),
                ("calibration-setup", setup),
                ("calibration-write-candidate", write_candidate),
                ("calibration-sample", sample),
            ] {
                collect_context_access_from_flow(flow, &nested_path(path, scope), output);
            }
        }
        DraftReferenceEvent::ComposedCall { symbol, flow, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { symbol, flow, .. } => {
            collect_context_access_from_flow(
                flow,
                &nested_path(path, &format!("call {symbol}")),
                output,
            );
        }
        _ => {}
    }
}

fn collect_context_access_from_flow(
    flow: &DraftReferenceFlow,
    path: &str,
    output: &mut Vec<ContextAccess>,
) {
    for event in &flow.events {
        collect_context_access_from_event(event, path, output);
    }
    if let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &flow.terminator
    {
        let condition = branch_expression(condition);
        collect_context_access_from_flow(
            taken,
            &nested_path(path, &format!("if {condition}")),
            output,
        );
        collect_context_access_from_flow(
            not_taken,
            &nested_path(path, &format!("if !({condition})")),
            output,
        );
    }
}

fn context_accesses_for_trace(trace: &FunctionAnalysis) -> Vec<ContextAccess> {
    let mut output = Vec::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_context_access_from_flow(flow, "entry", &mut output);
    } else {
        for event in &trace.reference_events {
            collect_context_access_from_event(event, "entry", &mut output);
        }
    }
    output.sort();
    output.dedup();
    output
}

fn context_fields_for_accesses(accesses: &[ContextAccess]) -> Vec<ContextField> {
    let mut fields = BTreeMap::<(u8, i32, u8), ContextField>::new();
    for access in accesses {
        let field = fields
            .entry((access.argument, access.offset, access.width))
            .or_insert_with(|| ContextField {
                argument: access.argument,
                offset: access.offset,
                width: access.width,
                reads: 0,
                writes: 0,
                write_mask: 0,
                paths: Vec::new(),
                write_values: Vec::new(),
            });
        match access.access {
            "read" => field.reads += 1,
            "write" => {
                field.writes += 1;
                field.write_mask |= access.write_mask.unwrap_or_default();
                if let Some(value) = access.value_pseudo.as_ref()
                    && !field.write_values.contains(value)
                {
                    field.write_values.push(value.clone());
                }
            }
            _ => unreachable!("context access has a closed access vocabulary"),
        }
        if !field.paths.contains(&access.path) {
            field.paths.push(access.path.clone());
        }
    }
    fields.into_values().collect()
}

fn mmio_write_masks(
    access: MemoryAccess,
    address: u32,
    width: u8,
    value: Option<&SymbolicValue>,
) -> [Option<u32>; 7] {
    if access != MemoryAccess::Write {
        return [None; 7];
    }
    let pattern = super::mmio_discovery::classify_write_bits(value, address, width);
    [
        Some(pattern.modified_mask(width)),
        Some(pattern.preserved_mask),
        Some(pattern.inverted_mask),
        Some(pattern.forced_zero_mask),
        Some(pattern.forced_one_mask),
        Some(pattern.read_derived_mask),
        Some(pattern.dynamic_mask),
    ]
}

struct MmioAccessDraft<'a> {
    address: u32,
    width: u8,
    register: &'a str,
    access: MemoryAccess,
    mode: &'static str,
    path: &'a str,
    address_expression: Option<String>,
    guard: Option<String>,
    value: Option<&'a SymbolicValue>,
}

fn push_mmio_access(output: &mut Vec<LinkedMmioAccess>, draft: MmioAccessDraft<'_>) {
    let MmioAccessDraft {
        address,
        width,
        register,
        access,
        mode,
        path,
        address_expression,
        guard,
        value,
    } = draft;
    let [
        modified_mask,
        preserved_mask,
        inverted_mask,
        forced_zero_mask,
        forced_one_mask,
        read_derived_mask,
        dynamic_mask,
    ] = mmio_write_masks(access, address, width, value);
    output.push(LinkedMmioAccess {
        ordinal: output.len(),
        address,
        width,
        register: register.to_owned(),
        access: match access {
            MemoryAccess::Read => "read",
            MemoryAccess::Write => "write",
        },
        mode,
        path: path.to_owned(),
        address_expression,
        guard,
        value: value.map(pseudo_value),
        modified_mask,
        preserved_mask,
        inverted_mask,
        forced_zero_mask,
        forced_one_mask,
        read_derived_mask,
        dynamic_mask,
    });
}

fn collect_mmio_access_from_event(
    event: &DraftReferenceEvent,
    path: &str,
    output: &mut Vec<LinkedMmioAccess>,
) {
    match event {
        DraftReferenceEvent::Observable(ObservableEvent::Memory {
            access,
            width,
            address,
            register,
            value,
        }) => push_mmio_access(
            output,
            MmioAccessDraft {
                address: *address,
                width: *width,
                register,
                access: *access,
                mode: "static",
                path,
                address_expression: None,
                guard: None,
                value: value.as_ref(),
            },
        ),
        DraftReferenceEvent::IndexedMmio {
            access,
            width,
            address,
            registers,
            guard,
            value,
        } => {
            let address_expression = Some(pseudo_value(address));
            let guard = guard
                .as_ref()
                .map(|guard| format!("{} < {}", pseudo_value(&guard.selector), guard.maximum));
            for register in registers {
                push_mmio_access(
                    output,
                    MmioAccessDraft {
                        address: register.address,
                        width: *width,
                        register: &register.name,
                        access: *access,
                        mode: "indexed-candidate",
                        path,
                        address_expression: address_expression.clone(),
                        guard: guard.clone(),
                        value: value.as_ref(),
                    },
                );
            }
        }
        DraftReferenceEvent::PollMmio {
            width,
            address,
            registers,
            guard,
            mask,
            expected,
        } => {
            let address_expression = Some(pseudo_value(address));
            let guard = guard.as_ref().map_or_else(
                || format!("value & {mask:#010x} == {expected:#010x}"),
                |guard| {
                    format!(
                        "{} < {}; value & {mask:#010x} == {expected:#010x}",
                        pseudo_value(&guard.selector),
                        guard.maximum
                    )
                },
            );
            for register in registers {
                let mut access = LinkedMmioAccess {
                    ordinal: output.len(),
                    address: register.address,
                    width: *width,
                    register: register.name.clone(),
                    access: "poll",
                    mode: "indexed-candidate",
                    path: path.to_owned(),
                    address_expression: address_expression.clone(),
                    guard: Some(guard.clone()),
                    value: None,
                    modified_mask: None,
                    preserved_mask: None,
                    inverted_mask: None,
                    forced_zero_mask: None,
                    forced_one_mask: None,
                    read_derived_mask: None,
                    dynamic_mask: None,
                };
                if registers.len() == 1 && address.as_constant() == Some(register.address) {
                    access.mode = "static";
                    access.address_expression = None;
                }
                output.push(access);
            }
        }
        DraftReferenceEvent::BoundedPoll {
            body, on_exhausted, ..
        } => {
            collect_mmio_access_from_flow(body, &nested_path(path, "bounded-poll"), output);
            if let Some(event) = on_exhausted.as_deref() {
                collect_mmio_access_from_event(event, &nested_path(path, "poll-exhausted"), output);
            }
        }
        DraftReferenceEvent::PollFlow { body, .. } => {
            collect_mmio_access_from_flow(body, &nested_path(path, "poll"), output);
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            for (scope, flow) in [
                ("calibration-initial-read", initial_read),
                ("calibration-setup", setup),
                ("calibration-write-candidate", write_candidate),
                ("calibration-sample", sample),
            ] {
                collect_mmio_access_from_flow(flow, &nested_path(path, scope), output);
            }
        }
        DraftReferenceEvent::ComposedCall { symbol, flow, .. }
        | DraftReferenceEvent::ComposedCallWithScratch { symbol, flow, .. } => {
            collect_mmio_access_from_flow(
                flow,
                &nested_path(path, &format!("call {symbol}")),
                output,
            );
        }
        _ => {}
    }
}

fn collect_mmio_access_from_flow(
    flow: &DraftReferenceFlow,
    path: &str,
    output: &mut Vec<LinkedMmioAccess>,
) {
    for event in &flow.events {
        collect_mmio_access_from_event(event, path, output);
    }
    if let DraftReferenceTerminator::Branch {
        condition,
        taken,
        not_taken,
    } = &flow.terminator
    {
        let condition = branch_expression(condition);
        collect_mmio_access_from_flow(
            taken,
            &nested_path(path, &format!("if {condition}")),
            output,
        );
        collect_mmio_access_from_flow(
            not_taken,
            &nested_path(path, &format!("if !({condition})")),
            output,
        );
    }
}

fn mmio_accesses_for_trace(trace: &FunctionAnalysis) -> Vec<LinkedMmioAccess> {
    let mut output = Vec::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_mmio_access_from_flow(flow, "entry", &mut output);
    } else {
        for event in &trace.reference_events {
            collect_mmio_access_from_event(event, "entry", &mut output);
        }
    }
    output
}

#[derive(Clone, Default)]
struct RenderState {
    mmio_reads: u32,
    memory_reads: u32,
}

fn indent(level: usize) -> String {
    "    ".repeat(level)
}

fn render_observable(
    event: &ObservableEvent,
    output: &mut String,
    level: usize,
    state: &mut RenderState,
) {
    let prefix = indent(level);
    match event {
        ObservableEvent::Memory {
            access,
            width,
            address,
            register,
            value,
        } => match access {
            MemoryAccess::Read => {
                writeln!(
                    output,
                    "{prefix}let read{} = mmio.read{width}({address:#010x}); // {register}",
                    state.mmio_reads
                )
                .unwrap();
                state.mmio_reads += 1;
            }
            MemoryAccess::Write => {
                let value = value
                    .as_ref()
                    .map_or_else(|| "unknown".to_owned(), pseudo_value);
                writeln!(
                    output,
                    "{prefix}mmio.write{width}({address:#010x}, {value}); // {register}"
                )
                .unwrap();
            }
        },
        ObservableEvent::Fence {
            fm,
            predecessor,
            successor,
        } => writeln!(
            output,
            "{prefix}fence(fm={fm:#x}, pred={predecessor:#x}, succ={successor:#x});"
        )
        .unwrap(),
    }
}

fn render_embedded_flow(
    label: &str,
    flow: &DraftReferenceFlow,
    output: &mut String,
    level: usize,
    state: &mut RenderState,
) {
    let prefix = indent(level);
    writeln!(output, "{prefix}// {label}").unwrap();
    for event in &flow.events {
        render_event(event, output, level, state);
    }
}

fn render_event(
    event: &DraftReferenceEvent,
    output: &mut String,
    level: usize,
    state: &mut RenderState,
) {
    let prefix = indent(level);
    match event {
        DraftReferenceEvent::Observable(event) => {
            render_observable(event, output, level, state);
        }
        DraftReferenceEvent::IndexedMmio {
            access,
            width,
            address,
            registers,
            guard,
            value,
        } => {
            let candidates = registers
                .iter()
                .map(|register| format!("{}@{:#010x}", register.name, register.address))
                .collect::<Vec<_>>()
                .join(", ");
            if let Some(guard) = guard {
                writeln!(
                    output,
                    "{prefix}assert!({} < {});",
                    pseudo_value(&guard.selector),
                    guard.maximum
                )
                .unwrap();
            }
            match access {
                MemoryAccess::Read => {
                    writeln!(
                        output,
                        "{prefix}let read{} = mmio.read{width}({}); // indexed: {candidates}",
                        state.mmio_reads,
                        pseudo_value(address)
                    )
                    .unwrap();
                    state.mmio_reads += 1;
                }
                MemoryAccess::Write => {
                    let value = value
                        .as_ref()
                        .map_or_else(|| "unknown".to_owned(), pseudo_value);
                    writeln!(
                        output,
                        "{prefix}mmio.write{width}({}, {value}); // indexed: {candidates}",
                        pseudo_value(address)
                    )
                    .unwrap();
                }
            }
        }
        DraftReferenceEvent::PollMmio {
            width,
            address,
            mask,
            expected,
            ..
        } => writeln!(
            output,
            "{prefix}while (mmio.read{width}({}) & {mask:#010x}) != {expected:#010x} {{ spin(); }}",
            pseudo_value(address)
        )
        .unwrap(),
        DraftReferenceEvent::BoundedPoll {
            maximum_attempts,
            body,
            repeat_while_mask,
            repeat_while_expected,
            on_exhausted,
        } => {
            writeln!(
                output,
                "{prefix}for attempt in 0..{maximum_attempts} {{ // repeat while result & {repeat_while_mask:#010x} == {repeat_while_expected:#010x}"
            )
            .unwrap();
            render_embedded_flow("poll body", body, output, level + 1, state);
            writeln!(output, "{prefix}}}").unwrap();
            if let Some(event) = on_exhausted.as_deref() {
                writeln!(output, "{prefix}if exhausted {{").unwrap();
                render_event(event, output, level + 1, state);
                writeln!(output, "{prefix}}}").unwrap();
            }
        }
        DraftReferenceEvent::PollFlow {
            body,
            exit_when_mask,
            exit_when_expected,
        } => {
            writeln!(
                output,
                "{prefix}loop {{ // exit when result & {exit_when_mask:#010x} == {exit_when_expected:#010x}"
            )
            .unwrap();
            render_embedded_flow("poll flow", body, output, level + 1, state);
            writeln!(output, "{prefix}}}").unwrap();
        }
        DraftReferenceEvent::SymmetricCalibrationSearch {
            attempts_per_direction,
            settle_micros,
            sample_shift,
            sample_mask,
            accepted_sample,
            initial_read,
            setup,
            write_candidate,
            sample,
            ..
        } => {
            writeln!(
                output,
                "{prefix}calibration_search(attempts={attempts_per_direction}, settle_us={settle_micros}, sample=(_ >> {sample_shift}) & {sample_mask:#x}, accepted={accepted_sample:#x}) {{"
            )
            .unwrap();
            for (label, flow) in [
                ("initial read", initial_read),
                ("setup", setup),
                ("write candidate", write_candidate),
                ("sample", sample),
            ] {
                render_embedded_flow(label, flow, output, level + 1, state);
            }
            writeln!(output, "{prefix}}}").unwrap();
        }
        DraftReferenceEvent::DelayMicros { micros } => {
            writeln!(output, "{prefix}delay_us({});", pseudo_value(micros)).unwrap();
        }
        DraftReferenceEvent::Memory {
            access,
            width,
            address,
            region,
            value,
        } => match access {
            MemoryAccess::Read => {
                if let Some((argument, offset)) = address.caller_memory_location() {
                    writeln!(
                        output,
                        "{prefix}let ramread{} = ctx{argument}.read{width}({offset:+#x}); // {region}",
                        state.memory_reads,
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "{prefix}let ramread{} = memory.read{width}({}); // {region}",
                        state.memory_reads,
                        pseudo_value(address)
                    )
                    .unwrap();
                }
                state.memory_reads += 1;
            }
            MemoryAccess::Write => {
                let value = value.as_ref().map_or_else(|| "unknown".to_owned(), pseudo_value);
                if let Some((argument, offset)) = address.caller_memory_location() {
                    writeln!(
                        output,
                        "{prefix}ctx{argument}.write{width}({offset:+#x}, {value}); // {region}"
                    )
                    .unwrap();
                } else {
                    writeln!(
                        output,
                        "{prefix}memory.write{width}({}, {value}); // {region}",
                        pseudo_value(address)
                    )
                    .unwrap();
                }
            }
        },
        DraftReferenceEvent::PrivateStackLoad {
            token,
            offset,
            width,
            signed,
        } => writeln!(
            output,
            "{prefix}let private_stack_read{token} = stack.load{width}({offset:+#x}, signed={signed});"
        )
        .unwrap(),
        DraftReferenceEvent::PrivateStackStore {
            offset,
            width,
            value,
        } => writeln!(
            output,
            "{prefix}stack.store{width}({offset:+#x}, {});",
            pseudo_value(value)
        )
        .unwrap(),
        DraftReferenceEvent::ExternalCall {
            token,
            table,
            function,
            arguments,
        } => {
            let function_spec = function.spec();
            writeln!(
                output,
                "{prefix}let external{token} = semantic.{}({}); // ABI {}+{:#x} {}, returns {}; replacement: {}",
                pseudo_identifier(function_spec.semantic.operation),
                pseudo_external_arguments(*function, arguments),
                table.spec().id,
                function_spec.offset,
                function_spec.c_name,
                function_spec.semantic.return_type,
                function_spec.semantic.replacement.unwrap_or("none"),
            )
            .unwrap();
        }
        DraftReferenceEvent::DiagnosticCall {
            function,
            arguments,
            ..
        } => writeln!(
            output,
            "{prefix}diagnostic.{function}({});",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::Call {
            token,
            target,
            arguments,
            ..
        } => writeln!(
            output,
            "{prefix}let call{token} = sub_{target:08x}({});",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::TailCall {
            target, arguments, ..
        } => writeln!(
            output,
            "{prefix}return sub_{target:08x}({}); // tail call",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::ComposedCall {
            token,
            symbol,
            arguments,
            result_modeled,
            ..
        } => {
            let callee = pseudo_identifier(symbol);
            if *result_modeled {
                writeln!(
                    output,
                    "{prefix}let call{token} = {callee}({});",
                    pseudo_arguments(arguments)
                )
                .unwrap();
            } else {
                writeln!(
                    output,
                    "{prefix}{callee}({}); // return value not modeled",
                    pseudo_arguments(arguments)
                )
                .unwrap();
            }
        }
        DraftReferenceEvent::ScratchCall {
            token,
            target,
            arguments,
            scratch_argument,
            scratch_size,
            ..
        } => writeln!(
            output,
            "{prefix}let call{token} = sub_{target:08x}_with_scratch(arg={scratch_argument}, size={scratch_size}, [{}]);",
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::ComposedCallWithScratch {
            token,
            symbol,
            arguments,
            result_modeled,
            scratch_argument,
            scratch_size,
            ..
        } => writeln!(
            output,
            "{prefix}{}{}({}); // scratch arg={scratch_argument} size={scratch_size}, result-modeled={result_modeled}",
            if *result_modeled {
                format!("let call{token} = ")
            } else {
                String::new()
            },
            pseudo_identifier(symbol),
            pseudo_arguments(arguments)
        )
        .unwrap(),
        DraftReferenceEvent::WideSignedDivide {
            token,
            dividend_low,
            dividend_high,
            divisor_low,
            divisor_high,
        } => writeln!(
            output,
            "{prefix}let wide_div{token} = sdiv64(low={}, high={}, divisor_low={}, divisor_high={});",
            pseudo_value(dividend_low),
            pseudo_value(dividend_high),
            pseudo_value(divisor_low),
            pseudo_value(divisor_high)
        )
        .unwrap(),
        DraftReferenceEvent::BranchDecision { condition, taken } => writeln!(
            output,
            "{prefix}// forced branch at {:#010x}: {} => {taken}",
            condition.site,
            branch_expression(condition)
        )
        .unwrap(),
    }
}

fn render_flow(
    flow: &DraftReferenceFlow,
    output: &mut String,
    level: usize,
    mut state: RenderState,
) {
    for event in &flow.events {
        render_event(event, output, level, &mut state);
    }
    let prefix = indent(level);
    match &flow.terminator {
        DraftReferenceTerminator::Return(value) => {
            writeln!(output, "{prefix}return {};", pseudo_value(value)).unwrap();
        }
        DraftReferenceTerminator::Branch {
            condition,
            taken,
            not_taken,
        } => {
            writeln!(
                output,
                "{prefix}if {} {{ // site {:#010x}",
                branch_expression(condition),
                condition.site
            )
            .unwrap();
            render_flow(taken, output, level + 1, state.clone());
            writeln!(output, "{prefix}}} else {{").unwrap();
            render_flow(not_taken, output, level + 1, state);
            writeln!(output, "{prefix}}}").unwrap();
        }
    }
}

fn render_pseudo(
    identity: &str,
    trace: &FunctionAnalysis,
    calls: &[LinkedCall],
    call_graph_blockers: &[String],
) -> String {
    let mut output = String::new();
    writeln!(output, "// vendor symbol: {identity}").unwrap();
    for blocker in &trace.blockers {
        writeln!(output, "// DIRECT-BLOCKER: {blocker}").unwrap();
    }
    for blocker in &trace.reference_blockers {
        writeln!(output, "// REFERENCE-BLOCKER: {blocker}").unwrap();
    }
    for blocker in call_graph_blockers {
        writeln!(output, "// CALL-GRAPH-BLOCKER: {blocker}").unwrap();
    }
    for call in calls {
        let site = call
            .site
            .map_or_else(|| "unknown-site".to_owned(), |site| format!("{site:#010x}"));
        writeln!(
            output,
            "// DIRECT-CALL {site}: {} {}{}",
            call.kind,
            call.target,
            if call.tail { " [tail]" } else { "" }
        )
        .unwrap();
    }
    writeln!(
        output,
        "fn {}(args: [u32; 16]) -> u32 {{",
        pseudo_identifier(identity)
    )
    .unwrap();
    writeln!(
        output,
        "    // argN denotes args[N]; ctxN denotes memory rooted at pointer argument N."
    )
    .unwrap();
    if let Some(flow) = trace.reference_flow.as_ref() {
        render_flow(flow, &mut output, 1, RenderState::default());
    } else {
        let mut state = RenderState::default();
        for event in &trace.reference_events {
            render_event(event, &mut output, 1, &mut state);
        }
        if trace.unresolved_branch.is_some() {
            writeln!(
                output,
                "    // control flow continues beyond the recovered prefix"
            )
            .unwrap();
        }
        writeln!(output, "    return {};", pseudo_value(&trace.return_value)).unwrap();
    }
    output.push_str("}\n");
    output
}

fn calls_for_trace(
    trace: &FunctionAnalysis,
    resolver: &ReferenceResolver,
    identities: &IrIdentityCatalog,
) -> Vec<LinkedCall> {
    let mut calls = BTreeSet::new();
    if let Some(flow) = trace.reference_flow.as_ref() {
        collect_calls_from_flow(flow, resolver, identities, &mut calls);
    } else {
        for event in &trace.reference_events {
            collect_calls_from_event(event, resolver, identities, &mut calls);
        }
    }
    calls.into_iter().collect()
}

pub(crate) fn build_linked_ir_for_source(
    resolver: &ReferenceResolver,
    symbol_prefix: &str,
    svd: &MmioRegisterMap,
    source: &str,
    namespace_identities: bool,
) -> LinkedIrReport {
    let mut functions = Vec::new();
    let identities = IrIdentityCatalog::new(resolver, namespace_identities.then_some(source));
    for symbol in resolver
        .symbols
        .iter()
        .filter(|symbol| symbol.name.starts_with(symbol_prefix))
    {
        let function_identity = identities.symbol(symbol);
        let binding = if resolver.symbol_is_exported(symbol) {
            "global-or-weak"
        } else {
            "local"
        };
        let DirectCallGraph {
            calls: direct_calls,
            blockers,
        } = explore_direct_calls(symbol, resolver, &identities, svd);
        let call_graph_blockers = blockers.into_iter().collect::<Vec<_>>();
        match resolver.trace_symbol(symbol, svd) {
            Ok(trace) => {
                let context_accesses = context_accesses_for_trace(&trace);
                let context_fields = context_fields_for_accesses(&context_accesses);
                let mmio_accesses = mmio_accesses_for_trace(&trace);
                let calls = if direct_calls.is_empty() {
                    calls_for_trace(&trace, resolver, &identities)
                } else {
                    direct_calls.into_iter().collect()
                };
                let flow_kind = if trace.reference_flow.is_some() {
                    "structured"
                } else if trace.is_reference_eligible() {
                    "linear"
                } else {
                    "partial"
                };
                let pseudo =
                    render_pseudo(&function_identity, &trace, &calls, &call_graph_blockers);
                functions.push(LinkedIrFunction {
                    source: source.to_owned(),
                    identity: function_identity.clone(),
                    member: symbol.member.clone(),
                    symbol: symbol.name.clone(),
                    binding,
                    address: symbol.addresses_resolved.then_some(symbol.address as u32),
                    object_offset: symbol.address as u32,
                    size: symbol.bytes.len(),
                    flow_kind,
                    complete: trace.is_reference_eligible(),
                    exact: trace.is_exact(),
                    return_value: trace.return_value.canonical(),
                    dependencies: trace
                        .reference_dependencies
                        .iter()
                        .map(|dependency| {
                            if namespace_identities {
                                format!("{source}::{dependency}")
                            } else {
                                dependency.clone()
                            }
                        })
                        .collect(),
                    calls,
                    mmio_accesses,
                    context_accesses,
                    context_fields,
                    call_graph_blockers,
                    direct_blockers: trace.blockers.clone(),
                    reference_blockers: trace.reference_blockers.clone(),
                    pseudo,
                });
            }
            Err(error) => functions.push(LinkedIrFunction {
                source: source.to_owned(),
                identity: function_identity.clone(),
                member: symbol.member.clone(),
                symbol: symbol.name.clone(),
                binding,
                address: symbol.addresses_resolved.then_some(symbol.address as u32),
                object_offset: symbol.address as u32,
                size: symbol.bytes.len(),
                flow_kind: "unavailable",
                complete: false,
                exact: false,
                return_value: "unknown".to_owned(),
                dependencies: Vec::new(),
                calls: direct_calls.into_iter().collect(),
                mmio_accesses: Vec::new(),
                context_accesses: Vec::new(),
                context_fields: Vec::new(),
                call_graph_blockers,
                direct_blockers: vec![error.to_string()],
                reference_blockers: Vec::new(),
                pseudo: format!(
                    "// vendor symbol: {function_identity}\n// DECODE-BLOCKER: {error}\nfn {}(args: [u32; 16]) -> u32 {{ unknown }}\n",
                    pseudo_identifier(&function_identity)
                ),
            }),
        }
    }

    summarize_linked_ir(functions)
}

pub(crate) fn merge_linked_ir(reports: Vec<LinkedIrReport>) -> LinkedIrReport {
    let functions = reports
        .into_iter()
        .flat_map(|report| report.functions)
        .collect();
    summarize_linked_ir(functions)
}

#[derive(Default)]
struct MmioRegisterAccumulator {
    names: BTreeSet<String>,
    read_shapes: usize,
    write_shapes: usize,
    poll_shapes: usize,
    static_shapes: usize,
    indexed_candidate_shapes: usize,
    functions: BTreeSet<String>,
}

fn summarize_linked_ir(mut functions: Vec<LinkedIrFunction>) -> LinkedIrReport {
    functions.sort_by(|left, right| left.identity.cmp(&right.identity));
    let mmio_functions = functions
        .iter()
        .filter(|function| !function.mmio_accesses.is_empty())
        .count();
    let mmio_access_shapes = functions
        .iter()
        .map(|function| function.mmio_accesses.len())
        .sum();
    let mut mmio_index = BTreeMap::<(u32, u8), MmioRegisterAccumulator>::new();
    for function in &functions {
        for access in &function.mmio_accesses {
            let entry = mmio_index
                .entry((access.address, access.width))
                .or_default();
            entry.names.insert(access.register.clone());
            match access.access {
                "read" => entry.read_shapes += 1,
                "write" => entry.write_shapes += 1,
                "poll" => entry.poll_shapes += 1,
                _ => unreachable!("linked MMIO access has a closed access vocabulary"),
            }
            match access.mode {
                "static" => entry.static_shapes += 1,
                "indexed-candidate" => entry.indexed_candidate_shapes += 1,
                _ => unreachable!("linked MMIO access has a closed address-mode vocabulary"),
            }
            entry.functions.insert(function.identity.clone());
        }
    }
    let mmio_registers = mmio_index
        .into_iter()
        .map(|((address, width), entry)| LinkedMmioRegister {
            address,
            width,
            names: entry.names.into_iter().collect(),
            read_shapes: entry.read_shapes,
            write_shapes: entry.write_shapes,
            poll_shapes: entry.poll_shapes,
            static_shapes: entry.static_shapes,
            indexed_candidate_shapes: entry.indexed_candidate_shapes,
            functions: entry.functions.into_iter().collect(),
        })
        .collect();
    let exported_functions = functions
        .iter()
        .filter(|function| function.binding == "global-or-weak")
        .count();
    let local_functions = functions
        .iter()
        .filter(|function| function.binding == "local")
        .count();
    let context_functions = functions
        .iter()
        .filter(|function| !function.context_accesses.is_empty())
        .count();
    let context_accesses = functions
        .iter()
        .map(|function| function.context_accesses.len())
        .sum();
    let context_fields = functions
        .iter()
        .map(|function| function.context_fields.len())
        .sum();
    let complete_functions = functions
        .iter()
        .filter(|function| function.complete)
        .count();
    let structured_functions = functions
        .iter()
        .filter(|function| function.flow_kind == "structured")
        .count();
    let internal_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "internal")
        .count();
    let external_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| matches!(call.kind, "external" | "diagnostic"))
        .count();
    let unresolved_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.kind == "unresolved")
        .count();
    let semantic_calls = functions
        .iter()
        .flat_map(|function| &function.calls)
        .filter(|call| call.semantic_operation.is_some())
        .count();
    let mut semantic_index =
        BTreeMap::<String, (usize, BTreeSet<String>, BTreeSet<String>, BTreeSet<String>)>::new();
    for function in &functions {
        for call in &function.calls {
            let Some(operation) = call.semantic_operation.as_ref() else {
                continue;
            };
            let entry = semantic_index.entry(operation.clone()).or_default();
            entry.0 += 1;
            entry.1.insert(function.identity.clone());
            entry.2.insert(call.target.clone());
            if let Some(replacement) = call.replacement_hint.as_ref() {
                entry.3.insert(replacement.clone());
            }
        }
    }
    let semantic_boundaries = semantic_index
        .into_iter()
        .map(
            |(operation, (call_shapes, functions, targets, replacement_hints))| SemanticBoundary {
                operation,
                call_shapes,
                functions: functions.into_iter().collect(),
                targets: targets.into_iter().collect(),
                replacement_hints: replacement_hints.into_iter().collect(),
            },
        )
        .collect();

    LinkedIrReport {
        functions,
        mmio_registers,
        mmio_functions,
        mmio_access_shapes,
        semantic_boundaries,
        semantic_calls,
        exported_functions,
        local_functions,
        context_functions,
        context_accesses,
        context_fields,
        complete_functions,
        structured_functions,
        internal_calls,
        external_calls,
        unresolved_calls,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn symbol(name: &str, address: u64, bytes: Vec<u8>) -> artifact::ArtifactSymbolDefinition {
        artifact::ArtifactSymbolDefinition {
            member: Some("member.o".to_owned()),
            name: name.to_owned(),
            address,
            bytes,
            addresses_resolved: false,
            memory_regions: Vec::new(),
            relocations: Vec::new(),
        }
    }

    fn empty_resolver() -> ReferenceResolver {
        ReferenceResolver {
            symbols: Vec::new(),
            symbols_by_address: BTreeMap::new(),
            symbol_ids: BTreeMap::new(),
            exported_symbol_keys: BTreeSet::new(),
            relocated_calls: BTreeMap::new(),
            pointer_context: direct::StructuralPointerContext::default(),
        }
    }

    #[test]
    fn direct_call_graph_survives_reference_summary_inlining() {
        let parent = symbol(
            "vendor_parent",
            0x1000,
            vec![
                0x97, 0x00, 0x00, 0x00, // auipc ra, 0
                0xe7, 0x80, 0x00, 0x00, // jalr ra, 0(ra)
                0x67, 0x80, 0x00, 0x00, // ret
            ],
        );
        let child = symbol(
            "vendor_child",
            0x2000,
            vec![0x67, 0x80, 0x00, 0x00], // ret
        );
        let child_id = 0x8000_0000;
        let resolver = ReferenceResolver {
            symbols: vec![parent.clone(), child.clone()],
            symbols_by_address: BTreeMap::from([(child_id, child)]),
            symbol_ids: BTreeMap::from([
                (
                    (parent.member.clone(), parent.name.clone(), parent.address),
                    0x8000_0001,
                ),
                (
                    (
                        Some("member.o".to_owned()),
                        "vendor_child".to_owned(),
                        0x2000,
                    ),
                    child_id,
                ),
            ]),
            exported_symbol_keys: BTreeSet::new(),
            relocated_calls: BTreeMap::from([(
                direct::StructuralCallSite::new(&parent, 0x1000),
                ("vendor_child".to_owned(), Some(child_id)),
            )]),
            pointer_context: direct::StructuralPointerContext::default(),
        };
        let map = MmioRegisterMap {
            registers: Vec::new(),
            windows: Vec::new(),
        };

        let identities = IrIdentityCatalog::new(&resolver, None);
        let graph = explore_direct_calls(&parent, &resolver, &identities, &map);
        let calls = graph.calls.into_iter().collect::<Vec<_>>();

        assert!(graph.blockers.is_empty(), "{:#?}", graph.blockers);
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].kind, "internal");
        assert_eq!(calls[0].target, "member.o:vendor_child");
        assert_eq!(calls[0].site, Some(0x1000));
    }

    #[test]
    fn external_call_keeps_reviewed_table_slot_semantics() {
        static ARGUMENTS: [crate::ExternalArgumentSpec; 1] = [crate::ExternalArgumentSpec {
            name: "micros",
            c_type: "u32",
            direction: crate::ExternalArgumentDirection::Input,
        }];
        static FUNCTIONS: [crate::ExternalFunctionSpec; 1] = [crate::ExternalFunctionSpec {
            id: "delay_us",
            offset: 0x20,
            c_name: "ets_delay_us",
            argument_count: 1,
            return_model: ExternalReturnModel::Constant(0),
            semantic: crate::ExternalSemanticSpec {
                operation: "time.delay-micros",
                arguments: &ARGUMENTS,
                return_type: "void",
                replacement: Some("Rust async timer"),
            },
        }];
        static TABLE: crate::ExternalTableSpec = crate::ExternalTableSpec {
            id: "wifi_osi",
            pointer_symbol: "g_wifi_osi_funcs",
            backing_symbol: "wifi_osi_funcs",
            version: 3,
            magic: 0x1234_5678,
            size: 0x100,
            magic_offset: 0,
            functions: &FUNCTIONS,
        };
        let event = DraftReferenceEvent::ExternalCall {
            token: 0,
            table: crate::ExternalTableRef::new(&TABLE),
            function: crate::ExternalFunctionRef::new(&FUNCTIONS[0]),
            arguments: vec![SymbolicValue::input(0)].into_boxed_slice(),
        };
        let mut calls = BTreeSet::new();
        let resolver = empty_resolver();
        let identities = IrIdentityCatalog::new(&resolver, None);
        let mut pseudo = String::new();

        collect_call_event(&event, &resolver, &identities, &mut calls);
        render_event(&event, &mut pseudo, 1, &mut RenderState::default());
        let call = calls.into_iter().next().unwrap();

        assert_eq!(call.kind, "external");
        assert_eq!(call.target, "wifi_osi::ets_delay_us");
        assert_eq!(call.arguments, [SymbolicValue::input(0).canonical()]);
        assert_eq!(
            call.semantic_operation.as_deref(),
            Some("time.delay-micros")
        );
        assert_eq!(call.replacement_hint.as_deref(), Some("Rust async timer"));
        assert_eq!(call.typed_arguments.len(), 1);
        assert_eq!(call.typed_arguments[0].name, "micros");
        assert_eq!(call.typed_arguments[0].c_type, "u32");
        assert_eq!(call.typed_arguments[0].direction, "input");
        assert!(
            pseudo.contains(
                "semantic.time_delay_micros(micros /* u32 Input */ = arg0); // ABI wifi_osi+0x20 ets_delay_us, returns void; replacement: Rust async timer"
            ),
            "{pseudo}"
        );
        assert!(
            call.semantics
                .as_deref()
                .is_some_and(|semantics| semantics.contains("version=3 slot=0x20 args=1")),
            "{:?}",
            call.semantics
        );
    }

    #[test]
    fn pseudo_value_renders_register_images_as_read_modify_write_expressions() {
        let value = SymbolicValue::RegisterImage {
            read_token: 3,
            address: 0x2010_7030,
            and_mask: 0xdfff_ffff,
            or_mask: 0x2000_0000,
        };

        assert_eq!(pseudo_value(&value), "((read3 & 0xdfffffff) | 0x20000000)");
    }

    #[test]
    fn pseudo_ir_keeps_a_named_call_and_structured_branch() {
        let callee_flow = DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Return(SymbolicValue::input(0)),
        };
        let flow = DraftReferenceFlow {
            events: vec![DraftReferenceEvent::ComposedCall {
                token: 0,
                symbol: "vendor_child".to_owned(),
                arguments: vec![SymbolicValue::input(0)].into_boxed_slice(),
                flow: Box::new(callee_flow),
                result_modeled: true,
            }],
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x1010,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::input(0),
                    right: SymbolicValue::Constant(0),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(1)),
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(2)),
                }),
            },
        };
        let trace = FunctionAnalysis {
            symbol: "vendor_parent".to_owned(),
            events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: vec!["vendor_child".to_owned()],
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Unknown,
            reference_flow: Some(flow),
            unresolved_branch: None,
        };

        let pseudo = render_pseudo("vendor_parent", &trace, &[], &[]);
        assert!(
            pseudo.contains("let call0 = vendor_child(arg0);"),
            "{pseudo}"
        );
        assert!(pseudo.contains("if arg0 == 0x00000000"), "{pseudo}");
        assert!(pseudo.contains("return 0x00000001;"), "{pseudo}");
        assert!(pseudo.contains("return 0x00000002;"), "{pseudo}");
    }

    #[test]
    fn context_map_recovers_argument_offsets_branch_paths_and_rmw_masks() {
        let write = DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 32,
            address: SymbolicValue::input(2).add_constant(4),
            region: "caller-owned ABI argument RAM".to_owned(),
            value: Some(SymbolicValue::MemoryImage {
                read_token: 0,
                and_mask: 0xffff_ffdf,
                or_mask: 0x20,
            }),
        };
        let trace = FunctionAnalysis {
            symbol: "update_context".to_owned(),
            events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Constant(0),
            reference_flow: Some(DraftReferenceFlow {
                events: Vec::new(),
                terminator: DraftReferenceTerminator::Branch {
                    condition: BranchCondition {
                        site: 0x1010,
                        operation: BranchOperation::NotEqual,
                        left: SymbolicValue::input(1),
                        right: SymbolicValue::Constant(0),
                    },
                    taken: Box::new(DraftReferenceFlow {
                        events: vec![write],
                        terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(1)),
                    }),
                    not_taken: Box::new(DraftReferenceFlow {
                        events: Vec::new(),
                        terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
                    }),
                },
            }),
            unresolved_branch: None,
        };

        let accesses = context_accesses_for_trace(&trace);
        let fields = context_fields_for_accesses(&accesses);
        let pseudo = render_pseudo("update_context", &trace, &[], &[]);

        assert_eq!(accesses.len(), 1);
        assert_eq!(accesses[0].argument, 2);
        assert_eq!(accesses[0].offset, 4);
        assert_eq!(accesses[0].write_mask, Some(0x20));
        assert_eq!(accesses[0].preserved_mask, Some(0xffff_ffdf));
        assert_eq!(accesses[0].forced_zero_mask, Some(0));
        assert_eq!(accesses[0].forced_one_mask, Some(0x20));
        assert!(accesses[0].path.contains("if arg1 != 0x00000000"));
        assert_eq!(fields.len(), 1);
        assert_eq!(fields[0].reads, 0);
        assert_eq!(fields[0].writes, 1);
        assert_eq!(fields[0].write_mask, 0x20);
        assert!(
            pseudo.contains("ctx2.write32(+0x4, ((ramread0 & 0xffffffdf) | 0x00000020));"),
            "{pseudo}"
        );
    }

    #[test]
    fn mmio_index_keeps_static_indexed_poll_and_write_bit_evidence() {
        let address = 0x2010_7030;
        let write_value = SymbolicValue::register_read(0, address, 32, false)
            .and(0xffff_fff0)
            .or(0x5);
        let trace = FunctionAnalysis {
            symbol: "touch_registers".to_owned(),
            events: Vec::new(),
            reference_events: Vec::new(),
            reference_dependencies: Vec::new(),
            blockers: Vec::new(),
            reference_blockers: Vec::new(),
            return_value: SymbolicValue::Constant(0),
            reference_flow: Some(DraftReferenceFlow {
                events: vec![
                    DraftReferenceEvent::Observable(ObservableEvent::Memory {
                        access: MemoryAccess::Write,
                        width: 32,
                        address,
                        register: "AGC.CONTROL".to_owned(),
                        value: Some(write_value),
                    }),
                    DraftReferenceEvent::IndexedMmio {
                        access: MemoryAccess::Read,
                        width: 32,
                        address: SymbolicValue::input(0).shift_left(2).add_constant(address),
                        registers: vec![
                            crate::IndexedMmioRegister {
                                address,
                                name: "AGC.CONTROL".to_owned(),
                            },
                            crate::IndexedMmioRegister {
                                address: address + 4,
                                name: "AGC.STATUS".to_owned(),
                            },
                        ],
                        guard: Some(crate::IndexedMmioGuard {
                            selector: SymbolicValue::input(0),
                            maximum: 2,
                        }),
                        value: None,
                    },
                    DraftReferenceEvent::PollMmio {
                        width: 32,
                        address: SymbolicValue::Constant(address + 4),
                        registers: vec![crate::IndexedMmioRegister {
                            address: address + 4,
                            name: "AGC.STATUS".to_owned(),
                        }],
                        guard: None,
                        mask: 1,
                        expected: 1,
                    },
                ],
                terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
            }),
            unresolved_branch: None,
        };

        let accesses = mmio_accesses_for_trace(&trace);

        assert_eq!(accesses.len(), 4);
        assert_eq!(
            accesses
                .iter()
                .map(|access| access.ordinal)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        let write = accesses
            .iter()
            .find(|access| access.access == "write")
            .unwrap();
        assert_eq!(write.mode, "static");
        assert_eq!(write.modified_mask, Some(0xf));
        assert_eq!(write.preserved_mask, Some(0xffff_fff0));
        assert_eq!(write.forced_zero_mask, Some(0xa));
        assert_eq!(write.forced_one_mask, Some(0x5));
        assert_eq!(
            accesses
                .iter()
                .filter(|access| access.mode == "indexed-candidate")
                .count(),
            2
        );
        let poll = accesses
            .iter()
            .find(|access| access.access == "poll")
            .unwrap();
        assert_eq!(poll.mode, "static");
        assert_eq!(
            poll.guard.as_deref(),
            Some("value & 0x00000001 == 0x00000001")
        );
    }

    #[test]
    fn duplicate_private_names_get_stable_address_qualified_ir_identities() {
        let first = artifact::ArtifactSymbolDefinition {
            member: None,
            name: "private_helper".to_owned(),
            address: 0x1000,
            bytes: vec![0x67, 0x80, 0x00, 0x00],
            addresses_resolved: true,
            memory_regions: Vec::new(),
            relocations: Vec::new(),
        };
        let second = artifact::ArtifactSymbolDefinition {
            address: 0x2000,
            ..first.clone()
        };
        let resolver = ReferenceResolver {
            symbols: vec![first.clone(), second.clone()],
            symbols_by_address: BTreeMap::from([
                (first.address as u32, first.clone()),
                (second.address as u32, second.clone()),
            ]),
            symbol_ids: BTreeMap::from([
                (
                    (None, first.name.clone(), first.address),
                    first.address as u32,
                ),
                (
                    (None, second.name.clone(), second.address),
                    second.address as u32,
                ),
            ]),
            exported_symbol_keys: BTreeSet::new(),
            relocated_calls: BTreeMap::new(),
            pointer_context: direct::StructuralPointerContext::default(),
        };
        let map = MmioRegisterMap {
            registers: Vec::new(),
            windows: Vec::new(),
        };

        let report = build_linked_ir_for_source(&resolver, "private_", &map, "primary", false);

        assert_eq!(report.exported_functions, 0);
        assert_eq!(report.local_functions, 2);
        assert_eq!(
            report
                .functions
                .iter()
                .map(|function| (function.identity.as_str(), function.binding))
                .collect::<Vec<_>>(),
            [
                ("private_helper@0x00001000", "local"),
                ("private_helper@0x00002000", "local"),
            ]
        );

        let project_report = merge_linked_ir(vec![
            build_linked_ir_for_source(&resolver, "private_", &map, "libphy", true),
            build_linked_ir_for_source(&resolver, "private_", &map, "rom", true),
        ]);
        assert_eq!(project_report.functions.len(), 4);
        assert_eq!(
            project_report
                .functions
                .iter()
                .map(|function| (function.source.as_str(), function.identity.as_str()))
                .collect::<Vec<_>>(),
            [
                ("libphy", "libphy::private_helper@0x00001000"),
                ("libphy", "libphy::private_helper@0x00002000"),
                ("rom", "rom::private_helper@0x00001000"),
                ("rom", "rom::private_helper@0x00002000"),
            ]
        );
    }
}
