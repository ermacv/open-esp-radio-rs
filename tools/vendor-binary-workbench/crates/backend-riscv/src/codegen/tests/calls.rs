//! Composed-call scoping, result escape and reviewed wide division.

use super::memory::bytes_to_word_events;
use super::*;

#[test]
fn renders_modeled_direct_platform_call_and_constant_guard() {
    let trace = FunctionAnalysis {
        symbol: "fixed_xtal".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::ModeledDirectCall {
            token: 0,
            site: 0x1000,
            function: open_radio_vendor_analysis_model::ModeledDirectCall {
                id: "fixed-xtal-40mhz".to_owned(),
                name: "rtc_clk_xtal_freq_get".to_owned(),
                argument_count: 0,
                return_model: ExternalReturnModel::Constant(40),
                operation: "clock.xtal-frequency.read".to_owned(),
                return_type: "u32".to_owned(),
                replacement_hint: Some("fixed target crystal contract".to_owned()),
                evidence: "test-target-contract".to_owned(),
            },
            arguments: Box::new([]),
        }],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(40),
        reference_flow: None,
        unresolved_branch: None,
    };

    let generated = generate_from_trace(&trace, "oracle.elf", "digest", None, &[]).unwrap();

    assert!(
        generated
            .source
            .contains("platform.direct_external_call(\"fixed-xtal-40mhz\", &[])")
    );
    assert!(
        generated
            .source
            .contains("assert_eq!(external_result0, 0x00000028_u32")
    );
}

#[test]
fn does_not_compact_a_composed_call_result_that_escapes_the_loop() {
    let reference_events = bytes_to_word_events(SymbolicValue::input(0), 0x1000_8000, 0);
    let trace = FunctionAnalysis {
        symbol: "escaping_call_result".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events,
        reference_dependencies: vec!["phy_byte_to_word".to_owned()],
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::CallResult(0),
        reference_flow: None,
        unresolved_branch: None,
    };

    let generated = generate_from_trace(&trace, "oracle.elf", "abc123", None, &[]).unwrap();

    assert!(!generated.source.contains("bytes-to-word loop"));
    assert!(
        generated
            .source
            .contains("Composed direct call: phy_byte_to_word")
    );
    assert!(generated.source.contains("exit_a0: Some(call_result0)"));
}

#[test]
fn nested_composed_call_arguments_do_not_shadow_the_parent_binding() {
    let leaf = || ResolvedReferenceFlow {
        events: Vec::new(),
        terminator: ResolvedReferenceTerminator::Return(SymbolicValue::input(0)),
    };
    let outer_flow = ResolvedReferenceFlow {
        events: vec![
            ResolvedReferenceEvent::ComposedCall {
                token: 0,
                symbol: "leaf".to_owned(),
                arguments: vec![SymbolicValue::input(0).add_constant(12)].into_boxed_slice(),
                flow: Box::new(leaf()),
                result_modeled: true,
            },
            ResolvedReferenceEvent::ComposedCall {
                token: 1,
                symbol: "leaf".to_owned(),
                arguments: vec![SymbolicValue::input(0).add_constant(16)].into_boxed_slice(),
                flow: Box::new(leaf()),
                result_modeled: true,
            },
        ],
        terminator: ResolvedReferenceTerminator::Return(SymbolicValue::CallResult(1)),
    };
    let program = ResolvedReferenceProgram {
        symbol: "wrapper".to_owned(),
        dependencies: vec!["outer".to_owned(), "leaf".to_owned()],
        body: ResolvedReferenceBody::Linear {
            events: vec![ResolvedReferenceEvent::ComposedCall {
                token: 0,
                symbol: "outer".to_owned(),
                arguments: vec![SymbolicValue::input(0)].into_boxed_slice(),
                flow: Box::new(outer_flow),
                result_modeled: true,
            }],
            return_value: SymbolicValue::CallResult(0),
        },
        exit_return_modeled: true,
    };

    let generated = generate(&program, "oracle.elf", "abc123", None, &[]).unwrap();

    assert!(
        generated.source.contains(
            "let call0_call0_arg0 = (call0_arg0 & 0xffffffff_u32).wrapping_add(0x0000000c_u32);"
        ),
        "{}",
        generated.source
    );
    assert!(generated.source.contains(
        "let call0_call1_arg0 = (call0_arg0 & 0xffffffff_u32).wrapping_add(0x00000010_u32);"
    ));
    assert!(
        !generated
            .source
            .contains("let call0_call1_arg0 = (call0_call0_arg0 & 0xffffffff_u32)")
    );
}

#[test]
fn renders_both_words_of_one_ordered_wide_division() {
    let trace = FunctionAnalysis {
        symbol: "wide_divide".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: vec![DraftReferenceEvent::WideSignedDivide {
            token: 0,
            dividend_low: SymbolicValue::input(0),
            dividend_high: SymbolicValue::input(1),
            divisor_low: SymbolicValue::input(2),
            divisor_high: SymbolicValue::input(3),
        }],
        reference_dependencies: vec!["__divdi3".to_owned()],
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::CallResult(0)
            .symbolic_bitxor(SymbolicValue::CallResult(SECONDARY_CALL_RESULT_TOKEN_FLAG)),
        reference_flow: None,
        unresolved_branch: None,
    };
    let generated = generate_from_trace(&trace, "rom.elf", "digest", None, &[]).unwrap();

    assert!(
        generated
            .source
            .contains("let (call_result0, call_result0_high) = riscv_div_i64_words(")
    );
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some((call_result0) ^ (call_result0_high)) }")
    );
    assert!(generated.source.contains(
        "assert!(divisor != 0, \"modeled __divdi3 precondition violated: divisor is zero\")"
    ));
}
