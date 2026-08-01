use super::*;
use crate::{DraftReferenceEvent, FunctionAnalysis};

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
