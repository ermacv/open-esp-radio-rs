//! Unit tests for linked-IR analysis and projection.

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

fn linked_test_function(
    source: &str,
    symbol: &str,
    binding: &'static str,
    calls: Vec<LinkedCall>,
) -> LinkedIrFunction {
    LinkedIrFunction {
        source: source.to_owned(),
        identity: format!("{source}::{symbol}"),
        selection: "symbol-prefix-root",
        member: None,
        symbol: symbol.to_owned(),
        binding,
        address: None,
        object_offset: 0,
        size: 4,
        flow_kind: "partial",
        complete: false,
        exact: false,
        return_value: "unknown".to_owned(),
        return_provenance: LinkedReturnProvenance {
            exact: false,
            known_zero_bits: 0,
            known_one_bits: 0,
            unknown_bits: u32::MAX,
            sources: Vec::new(),
        },
        dependencies: Vec::new(),
        calls,
        direct_mmio_predicates: Vec::new(),
        mmio_accesses: Vec::new(),
        delays: Vec::new(),
        context_accesses: Vec::new(),
        context_fields: Vec::new(),
        memory_accesses: Vec::new(),
        memory_fields: Vec::new(),
        scenario_suggestions: Vec::new(),
        effect_summary: LinkedEffectSummary::default(),
        call_graph_diagnostics: Vec::new(),
        direct_diagnostics: Vec::new(),
        reference_diagnostics: Vec::new(),
        decode_blockers: Vec::new(),
        pseudo: format!("// vendor symbol: {source}::{symbol}\n"),
    }
}

fn projected_argument(
    position: usize,
    name: &str,
    c_type: &str,
    value: &str,
) -> LinkedProjectedCallArgument {
    LinkedProjectedCallArgument {
        position,
        name: name.to_owned(),
        c_type: c_type.to_owned(),
        direction: "input",
        value: value.to_owned(),
        binding: "constant-or-symbolic",
        root_argument: None,
        root_offset: None,
    }
}

fn projected_semantic_action(
    operation: &str,
    arguments: Vec<LinkedProjectedCallArgument>,
    event_dispatch: Option<LinkedEventDispatchContract>,
) -> LinkedProjectedSemanticAction {
    LinkedProjectedSemanticAction {
        site_path: vec![Some(0x10)],
        operation: operation.to_owned(),
        target: "semantic::dispatch".to_owned(),
        contract: Some(LinkedSemanticContract {
            source: "test-reviewed-contract",
            id: format!("test::{operation}"),
            evidence: "unit-test".to_owned(),
            event_dispatch,
        }),
        replacement_hint: None,
        origin: "rom::irq_handler".to_owned(),
        path: "rom::irq_handler --semantic@0x00000010--> semantic::dispatch".to_owned(),
        site: Some(0x10),
        argument_shapes: 1,
        arguments,
        guard_scopes: Some(Vec::new()),
    }
}

fn event_dispatch_contract(
    mechanism: &'static str,
    execution_context: &'static str,
    receiver: Option<&'static str>,
    argument_roles: &[(&'static str, &'static str)],
) -> LinkedEventDispatchContract {
    LinkedEventDispatchContract {
        mechanism,
        execution_context,
        receiver,
        argument_roles: argument_roles
            .iter()
            .map(|&(role, argument)| LinkedEventDispatchArgumentRole { role, argument })
            .collect(),
    }
}

mod dispatch_and_calls;
mod flow_and_mmio;
mod guard_provenance;
mod project_summary;
mod recursion_and_delay;
