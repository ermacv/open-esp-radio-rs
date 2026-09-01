//! Unit tests for linked-IR analysis and projection.

use super::*;

const TEST_ARTIFACT_SHA256: &str =
    "1111111111111111111111111111111111111111111111111111111111111111";
const TEST_OTHER_ARTIFACT_SHA256: &str =
    "2222222222222222222222222222222222222222222222222222222222222222";

fn symbol(name: &str, address: u64, bytes: Vec<u8>) -> artifact::ArtifactSymbolDefinition {
    artifact::ArtifactSymbolDefinition {
        member: Some("member.o".to_owned()),
        name: name.to_owned(),
        address,
        bytes,
        addresses_resolved: false,
        memory_regions: Default::default(),
        relocations: Vec::new(),
    }
}

fn empty_resolver() -> ReferenceResolver {
    ReferenceResolver {
        symbols: Vec::new(),
        symbols_by_address: BTreeMap::new(),
        symbol_ids: BTreeMap::new(),
        exported_symbol_keys: BTreeSet::new(),
        relocated_calls: direct::StructuralRelocatedCalls::new(),
        pointer_context: direct::StructuralPointerContext::default(),
        data_symbols: Vec::new(),
        data_objects: Vec::new(),
        projected_direct_semantics: BTreeMap::new(),
        projected_origins: BTreeMap::new(),
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
        artifact_sha256: TEST_ARTIFACT_SHA256.to_owned(),
        identity: format!("{source}::{symbol}"),
        selection: "symbol-prefix-root",
        member: None,
        symbol: symbol.to_owned(),
        binding,
        address: None,
        object_offset: 0,
        size: 4,
        flow_kind: "partial",
        loops: Vec::new(),
        completeness: LinkedFunctionCompleteness {
            body_complete: false,
            call_targets_complete: false,
            transitive_effects_complete: false,
            executable_complete: false,
        },
        exact: false,
        return_value: "unknown".to_owned(),
        return_provenance: LinkedReturnProvenance {
            exact: false,
            known_zero_bits: 0,
            known_one_bits: 0,
            unknown_bits: u32::MAX,
            sources: Vec::new(),
        },
        return_frontier: None,
        call_result_frontiers: Vec::new(),
        dependencies: Vec::new(),
        projected_relocations: Vec::new(),
        local_value_flow: Vec::new(),
        indexed_dispatches: Vec::new(),
        calls,
        direct_mmio_predicates: Vec::new(),
        mmio_accesses: Vec::new(),
        instruction_effects: Vec::new(),
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

#[test]
fn schema_v66_requires_artifact_provenance_and_frontier_fields() {
    let render = || {
        crate::artifacts::render_linked_ir_fixture(
            vec![linked_test_function("rom", "worker", "global", Vec::new())],
            Vec::new(),
        )
    };
    let mut missing: serde_json::Value = serde_json::from_str(&render()).unwrap();
    missing["functions"][0]
        .as_object_mut()
        .expect("function object")
        .remove("artifact_sha256");
    let error = crate::artifacts::parse_linked_ir(&missing.to_string()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("missing field `artifact_sha256`"),
        "{error}"
    );

    let mut mismatched: serde_json::Value = serde_json::from_str(&render()).unwrap();
    mismatched["functions"][0]["artifact_sha256"] =
        serde_json::Value::String(TEST_OTHER_ARTIFACT_SHA256.to_owned());
    let error = crate::artifacts::parse_linked_ir(&mismatched.to_string()).unwrap_err();
    assert!(
        error.to_string().contains("undeclared source artifact"),
        "{error}"
    );

    let mut missing: serde_json::Value = serde_json::from_str(&render()).unwrap();
    missing["functions"][0]
        .as_object_mut()
        .expect("function object")
        .remove("return_frontier");
    let error = crate::artifacts::parse_linked_ir(&missing.to_string()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("missing field `return_frontier`"),
        "{error}"
    );

    let mut missing: serde_json::Value = serde_json::from_str(&render()).unwrap();
    missing["functions"][0]
        .as_object_mut()
        .expect("function object")
        .remove("call_result_frontiers");
    let error = crate::artifacts::parse_linked_ir(&missing.to_string()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("missing field `call_result_frontiers`"),
        "{error}"
    );

    let mut unknown: serde_json::Value = serde_json::from_str(&render()).unwrap();
    unknown["functions"][0]["return_frontier"] = serde_json::json!({
        "structurally_complete": false,
        "leaves": [],
        "fail_stops": [],
        "blockers": ["fixture structural blocker"],
        "legacy_text_guard": "arg0 == 0"
    });
    let error = crate::artifacts::parse_linked_ir(&unknown.to_string()).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("unknown field `legacy_text_guard`"),
        "{error}"
    );
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
            body_policy: "opaque-boundary",
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
