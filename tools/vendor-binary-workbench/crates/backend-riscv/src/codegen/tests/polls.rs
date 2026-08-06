//! Bounded-poll rendering and exhaustion diagnostics.

use super::*;

#[test]
fn renders_a_compact_bounded_poll_with_an_exhaustion_diagnostic() {
    let mut call_arguments: Rv32CallArguments = core::array::from_fn(|_| SymbolicValue::Unknown);
    call_arguments[0] = SymbolicValue::input(0);
    let mut diagnostic_arguments: [SymbolicValue; 8] =
        core::array::from_fn(|_| SymbolicValue::Unknown);
    diagnostic_arguments[0] = SymbolicValue::Constant(0x2f84_d9cc);
    let trace = FunctionAnalysis {
        symbol: "bounded_poll".to_owned(),
        events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: vec!["poll_read".to_owned(), "ets_printf".to_owned()],
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(DraftReferenceFlow {
            events: vec![DraftReferenceEvent::BoundedPoll {
                maximum_attempts: 100,
                body: Box::new(DraftReferenceFlow {
                    events: vec![
                        DraftReferenceEvent::DelayMicros {
                            micros: SymbolicValue::Constant(20),
                        },
                        DraftReferenceEvent::ComposedCall {
                            token: 0,
                            symbol: "poll_read".to_owned(),
                            arguments: Box::new(call_arguments),
                            flow: Box::new(DraftReferenceFlow {
                                events: Vec::new(),
                                terminator: DraftReferenceTerminator::Return(SymbolicValue::input(
                                    0,
                                )),
                            }),
                            result_modeled: true,
                        },
                    ],
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::CallResult(0)),
                }),
                repeat_while_mask: u32::MAX,
                repeat_while_expected: 0,
                on_exhausted: Some(Box::new(DraftReferenceEvent::DiagnosticCall {
                    function: "ets_printf".to_owned(),
                    argument_count: 1,
                    arguments: Box::new(diagnostic_arguments),
                })),
            }],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::Unknown),
        }),
        unresolved_branch: None,
    };

    let generated = generate_from_trace(&trace, "rom.elf", "digest", None, &[]).unwrap();
    assert!(
        generated
            .source
            .contains("for bounded_poll_attempt0 in 0..100_u16")
    );
    assert!(generated.source.contains("io.delay_micros(0x00000014_u32)"));
    assert!(
        generated
            .source
            .contains("if bounded_poll_value0 & 0xffffffff_u32 != 0x00000000_u32 { break; }")
    );
    assert!(
        generated
            .source
            .contains("platform.diagnostic_call(\"ets_printf\", &[0x2f84d9cc_u32])")
    );
}
