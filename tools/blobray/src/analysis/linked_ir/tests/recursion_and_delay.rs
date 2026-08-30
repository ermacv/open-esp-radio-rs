//! Recursive summaries and delay inventory.

use super::*;

#[test]
fn recursive_effect_summary_reaches_a_fixed_point() {
    let internal = |target: &str| LinkedCall {
        kind: "internal",
        target: target.to_owned(),
        site: Some(0),
        direct: true,
        tail: false,
        result_modeled: false,
        result_provenance: None,
        execution_model: None,
        semantics: None,
        semantic_operation: None,
        semantic_contract: None,
        replacement_hint: None,
        project_symbol: None,
        project_candidates: Vec::new(),
        trampoline: None,
        argument_shapes: 1,
        arguments: vec!["arg0".to_owned()],
        argument_exact: vec![true],
        argument_result_provenance: Vec::new(),
        argument_bindings: vec![LinkedArgumentBinding {
            position: 0,
            caller_argument: 0,
            offset: 0,
            expression: "arg0".to_owned(),
        }],
        typed_arguments: Vec::new(),
        guard_paths: None,
    };
    let mut first = linked_test_function(
        "rom",
        "first",
        "global-or-weak",
        vec![internal("rom::second")],
    );
    first.completeness.body_complete = true;
    first.completeness.call_targets_complete = true;
    first.completeness.executable_complete = true;
    first.context_accesses.push(ContextAccess {
        argument: 0,
        offset: 0,
        access: "read",
        width: 32,
        path: "entry".to_owned(),
        value: None,
        value_pseudo: None,
        write_mask: None,
        preserved_mask: None,
        forced_zero_mask: None,
        forced_one_mask: None,
    });
    let mut second = linked_test_function(
        "rom",
        "second",
        "global-or-weak",
        vec![internal("rom::first")],
    );
    second.completeness.body_complete = true;
    second.completeness.call_targets_complete = true;
    second.completeness.executable_complete = true;

    let report = summarize_linked_ir(vec![first, second]);
    assert_eq!(report.closed_effect_summaries, 2);
    assert_eq!(report.recursive_effect_summaries, 2);
    for function in &report.functions {
        assert!(function.effect_summary.call_graph_closed);
        assert!(function.effect_summary.recursive);
        assert_eq!(function.effect_summary.reachable_function_count, 1);
        assert_eq!(function.effect_summary.max_depth, 1);
        assert!(!function.effect_summary.context_projection_complete);
        assert!(
            function
                .effect_summary
                .context_projection_blockers
                .iter()
                .any(|blocker| blocker.starts_with("recursive context projection stopped:"))
        );
    }
}

#[test]
fn delay_inventory_preserves_nested_path_and_constant() {
    let flow = DraftReferenceFlow {
        events: vec![DraftReferenceEvent::ComposedCall {
            token: 1,
            site: 0x1020,
            symbol: "delay_child".to_owned(),
            direct: true,
            tail: false,
            arguments: Box::new([]),
            flow: Box::new(DraftReferenceFlow {
                events: vec![DraftReferenceEvent::DelayMicros {
                    micros: SymbolicValue::Constant(20),
                }],
                terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
            }),
            result_modeled: true,
        }],
        terminator: DraftReferenceTerminator::Return(SymbolicValue::Constant(0)),
    };
    let mut delays = Vec::new();
    collect_delays_from_flow(&flow, "entry", &mut delays);

    assert_eq!(delays.len(), 1);
    assert_eq!(delays[0].ordinal, 0);
    assert_eq!(delays[0].path, "entry / call delay_child");
    assert_eq!(delays[0].micros, "const:0x00000014");
    assert_eq!(delays[0].constant_micros, Some(20));
}
