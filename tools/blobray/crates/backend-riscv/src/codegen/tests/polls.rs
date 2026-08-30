//! Bounded-poll rendering and exhaustion diagnostics.

use super::*;

#[test]
fn resolves_a_bounded_poll_with_an_exhaustion_diagnostic() {
    let mut call_arguments: Rv32CallArguments = core::array::from_fn(|_| SymbolicValue::Unknown);
    call_arguments[0] = SymbolicValue::input(0);
    let mut diagnostic_arguments: [SymbolicValue; 8] =
        core::array::from_fn(|_| SymbolicValue::Unknown);
    diagnostic_arguments[0] = SymbolicValue::Constant(0x2f84_d9cc);
    let trace = FunctionAnalysis {
        symbol: "bounded_poll".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
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
                    site: 0x1010,
                    function: "ets_printf".to_owned(),
                    argument_count: 1,
                    arguments: Box::new(diagnostic_arguments),
                })),
            }],
            terminator: DraftReferenceTerminator::Return(SymbolicValue::Unknown),
        }),
        unresolved_branch: None,
    };

    let program = ResolvedReferenceProgram::try_from(&trace).unwrap();
    let ResolvedReferenceBody::Flow(flow) = program.body else {
        panic!("flow trace resolved to a linear body");
    };
    let [
        ResolvedReferenceEvent::BoundedPoll {
            maximum_attempts: 100,
            body,
            on_exhausted: Some(on_exhausted),
            ..
        },
    ] = flow.events.as_slice()
    else {
        panic!("bounded poll was not retained as one resolved event");
    };
    assert!(matches!(
        body.events.as_slice(),
        [
            ResolvedReferenceEvent::DelayMicros { .. },
            ResolvedReferenceEvent::ComposedCall {
                result_modeled: true,
                ..
            }
        ]
    ));
    assert!(matches!(
        on_exhausted.as_ref(),
        ResolvedReferenceEvent::DiagnosticCall {
            argument_count: 1,
            ..
        }
    ));
}
