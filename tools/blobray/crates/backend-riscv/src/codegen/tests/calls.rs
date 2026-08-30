//! Composed-call scoping, result escape and reviewed wide division.

use super::memory::bytes_to_word_events;
use super::*;

#[test]
fn resolves_modeled_direct_call_and_constant_result() {
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

    let program = ResolvedReferenceProgram::try_from(&trace).unwrap();
    let ResolvedReferenceBody::Linear {
        events,
        return_value,
    } = program.body
    else {
        panic!("linear trace resolved to a flow body");
    };
    assert_eq!(return_value, SymbolicValue::Constant(40));
    assert!(matches!(
        events.as_slice(),
        [ResolvedReferenceEvent::ModeledDirectCall {
            function: open_radio_vendor_analysis_model::ModeledDirectCall {
                return_model: ExternalReturnModel::Constant(40),
                ..
            },
            arguments,
            ..
        }] if arguments.is_empty()
    ));
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

    let program = ResolvedReferenceProgram::try_from(&trace).unwrap();
    let ResolvedReferenceBody::Linear {
        events,
        return_value,
    } = program.body
    else {
        panic!("linear trace resolved to a flow body");
    };
    assert_eq!(return_value, SymbolicValue::CallResult(0));
    assert_eq!(events.len(), 2);
    assert!(matches!(
        &events[0],
        ResolvedReferenceEvent::ComposedCall {
            token: 0,
            result_modeled: true,
            ..
        }
    ));
    assert!(matches!(
        &events[1],
        ResolvedReferenceEvent::Memory {
            access: MemoryAccess::Write,
            value: Some(SymbolicValue::CallResult(0)),
            ..
        }
    ));
}

#[test]
fn resolves_both_words_of_one_ordered_wide_division() {
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
    let program = ResolvedReferenceProgram::try_from(&trace).unwrap();
    let ResolvedReferenceBody::Linear {
        events,
        return_value,
    } = program.body
    else {
        panic!("linear trace resolved to a flow body");
    };
    assert!(program.exit_return_modeled);
    assert!(matches!(
        events.as_slice(),
        [ResolvedReferenceEvent::WideSignedDivide { token: 0, .. }]
    ));
    assert_eq!(return_value, trace.return_value);
}
