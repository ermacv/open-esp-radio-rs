use super::*;
use crate::Rv32CallArguments;
use crate::{DraftReferenceEvent, DraftReferenceFlow, DraftReferenceTerminator, FunctionAnalysis};

fn generate_from_trace(
    trace: &FunctionAnalysis,
    artifact: &str,
    artifact_sha256: &str,
    member: Option<&str>,
    companions: &[(String, String)],
) -> Result<GeneratedReference, String> {
    let program = ResolvedReferenceProgram::try_from(trace)?;
    generate(&program, artifact, artifact_sha256, member, companions)
}

#[test]
fn groups_shifted_argument_bits_into_a_readable_expression() {
    let value = SymbolicValue::input(0).and(1).shift_left(5);
    assert_eq!(
        render_value(&value, &[], &[], 0).unwrap(),
        "(args[0] << 5) & 0x00000020_u32"
    );
}

#[test]
fn validates_the_address_behind_a_read_token() {
    let value = SymbolicValue::RegisterImage {
        read_token: 0,
        address: 0x2010_7030,
        and_mask: u32::MAX,
        or_mask: 0,
    };
    assert!(render_value(&value, &[0x2010_7030], &[], 0).is_ok());
    assert!(render_value(&value, &[0x2010_7034], &[], 0).is_err());
}

#[test]
fn distinguishes_static_and_indexed_read_tokens() {
    let value = SymbolicValue::IndexedRegisterImage {
        read_token: 0,
        and_mask: u32::MAX,
        or_mask: 0,
    };
    let arguments = core::array::from_fn(|index| format!("args[{index}]"));

    assert!(
        render_value_scoped(&value, &[MmioReadAddress::Indexed], 0, &[], 0, &arguments,).is_ok()
    );
    assert!(
        render_value_scoped(
            &value,
            &[MmioReadAddress::Static(0x2010_7030)],
            0,
            &[],
            0,
            &arguments,
        )
        .is_err()
    );
}

#[test]
fn renders_external_results_through_exact_riscv_arithmetic() {
    let value = SymbolicValue::expression(
        crate::ExpressionOperation::RemainderUnsigned,
        SymbolicValue::ExternalResult(0),
        SymbolicValue::Constant(11),
    )
    .add_constant(0xfa)
    .shift_left(21);

    let rendered = render_value(&value, &[], &[], 1).unwrap();
    assert!(rendered.contains("riscv_remu(external_result0, 0x0000000b_u32)"));
    assert!(rendered.contains("wrapping_add(0x000000fa_u32)"));
    assert!(rendered.contains("wrapping_shl"));
    assert!(render_value(&value, &[], &[], 0).is_err());
}

#[test]
fn renders_dynamic_arithmetic_shift_with_rv32_masking() {
    let value = SymbolicValue::expression(
        crate::ExpressionOperation::ShiftRightArithmetic,
        SymbolicValue::Constant((-0x81_i32) as u32),
        SymbolicValue::input(0),
    );

    assert_eq!(
        render_value(&value, &[], &[], 0).unwrap(),
        "((0xffffff7f_u32) as i32).wrapping_shr((args[0] & 0xffffffff_u32) & 31) as u32"
    );
}

#[test]
fn signed_branch_casts_the_complete_rendered_expression() {
    let condition = BranchCondition {
        site: 0,
        operation: BranchOperation::LessSigned,
        left: SymbolicValue::input(1),
        right: SymbolicValue::Constant(0),
    };

    assert_eq!(
        render_condition(&condition, &RenderState::default()).unwrap(),
        "((args[1] & 0xffffffff_u32) as i32) < ((0x00000000_u32) as i32)"
    );
}

#[test]
fn generates_a_self_contained_ordered_reference() {
    let trace = FunctionAnalysis {
        symbol: "phy-example".to_owned(),
        events: vec![
            ObservableEvent::Memory {
                access: MemoryAccess::Read,
                width: 32,
                address: 0x2010_7030,
                register: "AGC.CONTROL".to_owned(),
                value: None,
            },
            ObservableEvent::Memory {
                access: MemoryAccess::Write,
                width: 32,
                address: 0x2010_7030,
                register: "AGC.CONTROL".to_owned(),
                value: Some(SymbolicValue::RegisterImage {
                    read_token: 0,
                    address: 0x2010_7030,
                    and_mask: 0xffff_fffe,
                    or_mask: 1,
                }),
            },
        ],
        reference_events: vec![
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Read,
                width: 32,
                address: 0x2010_7030,
                register: "AGC.CONTROL".to_owned(),
                value: None,
            }),
            DraftReferenceEvent::Observable(ObservableEvent::Memory {
                access: MemoryAccess::Write,
                width: 32,
                address: 0x2010_7030,
                register: "AGC.CONTROL".to_owned(),
                value: Some(SymbolicValue::RegisterImage {
                    read_token: 0,
                    address: 0x2010_7030,
                    and_mask: 0xffff_fffe,
                    or_mask: 1,
                }),
            }),
            DraftReferenceEvent::DelayMicros {
                micros: SymbolicValue::Constant(7),
            },
        ],
        reference_dependencies: vec!["child_leaf".to_owned()],
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::input(0),
        reference_flow: None,
        unresolved_branch: None,
    };
    let generated = generate_from_trace(
        &trace,
        "oracle.elf",
        "abc123",
        None,
        &[("rom.elf".to_owned(), "def456".to_owned())],
    )
    .unwrap();

    assert!(generated.exit_a0_modeled);
    assert!(generated.source.contains("pub trait ReferenceIo"));
    assert!(generated.source.contains("// Companion artifact: rom.elf"));
    assert!(generated.source.contains("// Companion SHA-256: def456"));
    assert!(
        generated
            .source
            .contains("// Composed direct-call dependency: child_leaf")
    );
    assert!(
        generated
            .source
            .contains("pub fn open_phy_reference_phy_example(")
    );
    assert!(
        generated
            .source
            .contains("let read0 = io.read(32, 0x20107030_u32);")
    );
    assert!(
        generated
            .source
            .contains("io.write(32, 0x20107030_u32, (read0 & 0xfffffffe_u32) | 0x00000001_u32);")
    );
    assert!(
        generated
            .source
            .contains("io.delay_micros(0x00000007_u32);")
    );
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some(args[0] & 0xffffffff_u32) }")
    );
}

#[test]
fn rejects_incomplete_control_flow_instead_of_emitting_a_partial_function() {
    let trace = FunctionAnalysis {
        symbol: "branchy".to_owned(),
        events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: vec!["control-flow instruction at 0x10".to_owned()],
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: None,
        unresolved_branch: None,
    };
    let error = generate_from_trace(&trace, "oracle.elf", "abc123", None, &[]).unwrap_err();
    assert!(error.contains("not eligible"));
    assert!(error.contains("control-flow"));
}

#[test]
fn preserves_ordered_elf_ram_reads_and_writes() {
    let address = 0x3fcd_0010;
    let trace = FunctionAnalysis {
        symbol: "state_leaf".to_owned(),
        events: Vec::new(),
        reference_events: vec![
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Read,
                width: 32,
                address: SymbolicValue::Constant(address),
                region: ".data".to_owned(),
                value: None,
            },
            DraftReferenceEvent::Memory {
                access: MemoryAccess::Write,
                width: 32,
                address: SymbolicValue::Constant(address),
                region: ".data".to_owned(),
                value: Some(SymbolicValue::MemoryImage {
                    read_token: 0,
                    and_mask: 0xffff_ff00,
                    or_mask: 0x55,
                }),
            },
        ],
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::MemoryImage {
            read_token: 0,
            and_mask: u32::MAX,
            or_mask: 0,
        },
        reference_flow: None,
        unresolved_branch: None,
    };
    let generated = generate_from_trace(&trace, "oracle.elf", "abc123", None, &[]).unwrap();

    let read = generated
        .source
        .find("let memory_read0 = memory.read(32, memory_address0);")
        .unwrap();
    let write = generated
        .source
        .find("memory.write(32, memory_address1, memory_value1);")
        .unwrap();
    assert!(
        generated
            .source
            .contains("let memory_address0 = 0x3fcd0010_u32;")
    );
    assert!(
        generated
            .source
            .contains("let memory_address1 = 0x3fcd0010_u32;")
    );
    assert!(read < write);
    assert!(
        generated
            .source
            .contains("(memory_read0 & 0xffffff00_u32) | 0x00000055_u32")
    );
    assert!(
        generated
            .source
            .contains("ReferenceOutcome { exit_a0: Some((memory_read0")
    );
}

fn unrolled_word_to_bytes_events(
    source: u32,
    destination: SymbolicValue,
    first_read_token: u32,
) -> Vec<DraftReferenceEvent> {
    let mut events = Vec::new();
    for byte in 0..4_u32 {
        let read_token = first_read_token + byte;
        events.push(DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 32,
            address: SymbolicValue::Constant(source),
            region: ".data".to_owned(),
            value: None,
        });
        events.push(DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 8,
            address: destination.clone().add_constant(byte),
            region: "caller-owned ABI argument RAM".to_owned(),
            value: Some(
                SymbolicValue::memory_read(read_token, 32, false)
                    .shift_right(byte * 8)
                    .and(0xff),
            ),
        });
    }
    events
}

fn little_endian_loader_flow() -> DraftReferenceFlow {
    let events = [1_u32, 0, 2, 3]
        .into_iter()
        .map(|offset| DraftReferenceEvent::Memory {
            access: MemoryAccess::Read,
            width: 8,
            address: SymbolicValue::input(0).add_constant(offset),
            region: "caller-owned ABI argument RAM".to_owned(),
            value: None,
        })
        .collect();
    let value = SymbolicValue::memory_read(1, 8, false)
        .symbolic_bitor(SymbolicValue::memory_read(0, 8, false).shift_left(8))
        .symbolic_bitor(SymbolicValue::memory_read(2, 8, false).shift_left(16))
        .symbolic_bitor(SymbolicValue::memory_read(3, 8, false).shift_left(24));
    DraftReferenceFlow {
        events,
        terminator: DraftReferenceTerminator::Return(value),
    }
}

fn bytes_to_word_events(
    source: SymbolicValue,
    destination: u32,
    token: u32,
) -> Vec<DraftReferenceEvent> {
    vec![
        DraftReferenceEvent::ComposedCall {
            token,
            symbol: "phy_byte_to_word".to_owned(),
            arguments: vec![source].into_boxed_slice(),
            flow: Box::new(little_endian_loader_flow()),
            result_modeled: true,
        },
        DraftReferenceEvent::Memory {
            access: MemoryAccess::Write,
            width: 32,
            address: SymbolicValue::Constant(destination),
            region: ".data".to_owned(),
            value: Some(SymbolicValue::CallResult(token)),
        },
    ]
}

#[test]
fn compacts_complete_word_to_bytes_groups_without_changing_the_access_shape() {
    let destination = SymbolicValue::input(0).add_constant(12);
    let mut reference_events = unrolled_word_to_bytes_events(0x1000_8000, destination.clone(), 0);
    reference_events.extend(unrolled_word_to_bytes_events(
        0x1000_8004,
        destination.add_constant(4),
        4,
    ));
    let trace = FunctionAnalysis {
        symbol: "word_to_bytes".to_owned(),
        events: Vec::new(),
        reference_events,
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(0),
        reference_flow: None,
        unresolved_branch: None,
    };

    let generated = generate_from_trace(&trace, "oracle.elf", "abc123", None, &[]).unwrap();

    assert!(
        generated
            .source
            .contains("Proven 8-byte CPU-RAM word-to-bytes loop")
    );
    assert!(
        generated
            .source
            .contains("for memory_transfer_word_offset0 in (0..8_u32).step_by(4)")
    );
    assert!(generated.source.contains("memory.read(32,"));
    assert!(generated.source.contains("memory.write(8,"));
    assert!(!generated.source.contains("memory.read(8,"));
}

#[test]
fn does_not_compact_a_memory_read_token_that_escapes_the_loop() {
    let reference_events = unrolled_word_to_bytes_events(0x1000_8000, SymbolicValue::input(0), 0);
    let trace = FunctionAnalysis {
        symbol: "escaping_word".to_owned(),
        events: Vec::new(),
        reference_events,
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::MemoryImage {
            read_token: 3,
            and_mask: u32::MAX,
            or_mask: 0,
        },
        reference_flow: None,
        unresolved_branch: None,
    };

    let generated = generate_from_trace(&trace, "oracle.elf", "abc123", None, &[]).unwrap();

    assert!(!generated.source.contains("word-to-bytes loop"));
    assert!(
        generated
            .source
            .contains("let memory_read3 = memory.read(32,")
    );
    let outcome = generated.source.rsplit("ReferenceOutcome").next().unwrap();
    assert!(outcome.contains("memory_read3"), "{outcome}");
}

#[test]
fn compacts_proven_little_endian_loaders_and_preserves_read_order() {
    let source = SymbolicValue::input(0).add_constant(12);
    let mut reference_events = bytes_to_word_events(source.clone(), 0x1000_8000, 0);
    reference_events.extend(bytes_to_word_events(source.add_constant(4), 0x1000_8004, 1));
    let trace = FunctionAnalysis {
        symbol: "bytes_to_word".to_owned(),
        events: Vec::new(),
        reference_events,
        reference_dependencies: vec!["phy_byte_to_word".to_owned(); 2],
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Constant(0),
        reference_flow: None,
        unresolved_branch: None,
    };

    let generated = generate_from_trace(&trace, "oracle.elf", "abc123", None, &[]).unwrap();

    assert!(
        generated
            .source
            .contains("Proven 8-byte CPU-RAM bytes-to-word loop")
    );
    let byte1 = generated.source.find("memory_transfer_byte1_0").unwrap();
    let byte0 = generated.source.find("memory_transfer_byte0_0").unwrap();
    let byte2 = generated.source.find("memory_transfer_byte2_0").unwrap();
    let byte3 = generated.source.find("memory_transfer_byte3_0").unwrap();
    assert!(byte1 < byte0 && byte0 < byte2 && byte2 < byte3);
    assert_eq!(
        generated
            .source
            .matches("Composed direct-call dependency: phy_byte_to_word")
            .count(),
        1
    );
}

#[test]
fn does_not_compact_a_composed_call_result_that_escapes_the_loop() {
    let reference_events = bytes_to_word_events(SymbolicValue::input(0), 0x1000_8000, 0);
    let trace = FunctionAnalysis {
        symbol: "escaping_call_result".to_owned(),
        events: Vec::new(),
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

    assert!(generated.source.contains(
        "let call0_call0_arg0 = (call0_arg0 & 0xffffffff_u32).wrapping_add(0x0000000c_u32);"
    ));
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
