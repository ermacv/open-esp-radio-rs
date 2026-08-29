//! Generated-source scaffolding, fail-closed flow and ordered-memory output.

use super::*;

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
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
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
            .contains("pub fn vendor_reference_phy_example(")
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
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
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
fn renders_conditional_fail_stop_as_a_diverging_platform_boundary() {
    let trace = FunctionAnalysis {
        symbol: "conditional_assert".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
        reference_events: Vec::new(),
        reference_dependencies: Vec::new(),
        blockers: Vec::new(),
        reference_blockers: Vec::new(),
        return_value: SymbolicValue::Unknown,
        reference_flow: Some(DraftReferenceFlow {
            events: Vec::new(),
            terminator: DraftReferenceTerminator::Branch {
                condition: BranchCondition {
                    site: 0x1002,
                    operation: BranchOperation::Equal,
                    left: SymbolicValue::input(4),
                    right: SymbolicValue::Constant(3),
                },
                taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::FailStop {
                        site: 0x100c,
                        function: "controller_assert".to_owned(),
                        argument_count: 1,
                        arguments: Box::new([SymbolicValue::input(4)]),
                    },
                }),
                not_taken: Box::new(DraftReferenceFlow {
                    events: Vec::new(),
                    terminator: DraftReferenceTerminator::Return(SymbolicValue::Unknown),
                }),
            },
        }),
        unresolved_branch: None,
    };

    let generated = generate_from_trace(&trace, "oracle.elf", "abc123", None, &[]).unwrap();

    assert!(
        generated
            .source
            .contains("fn fail_stop(&mut self, function: &str, arguments: &[u32]) -> !;")
    );
    assert!(
        generated
            .source
            .contains("if (args[4] & 0xffffffff_u32) == (0x00000003_u32)")
    );
    assert!(
        generated
            .source
            .contains("platform.fail_stop(\"controller_assert\", &[args[4] & 0xffffffff_u32])")
    );
}

#[test]
fn preserves_ordered_elf_ram_reads_and_writes() {
    let address = 0x3fcd_0010;
    let trace = FunctionAnalysis {
        symbol: "state_leaf".to_owned(),
        events: Vec::new(),
        located_events: Vec::new(),
        located_reference_events: Vec::new(),
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
